> 历史设计草案，保留作背景资料；当前实现与不变量以 `ARCHITECTURE.zh.md` 为准。

# wjsel 设计：把 simdjson 的位并行搬进 HTML 选择器（CPU 先行，GPU 后置）

仿照 simdjson 的位并行做 HTML parser 和 selector.

1. 为何不做 GPU？
   - cuJSON 指出 8MiB 是性能转折点，大部分单体 HTML < 3MiB.
   - 并非没有价值，但是离线高并发
   - 之后做，不浪费现在带宽

2. Python binding `wjsel`
   - `parsel` 替代 `Selector(text=...).css(...).xpath(...)`
   - 临时提供 `parallel=True` 选项，未来 Python 自己可能支持多线程

3. 原理：把 HTML 选择拆成两段
   - stage-1: 64 字节块、`vpshufb` 分类、`pclmulqdq` prefi-XOR、`tzcnt` flatten -> `MarkupIndex`
   - stage-2: 不建浏览器 DOM, 规范化为嵌套 Dyck 序列，stack 先序 `FlatDoc` + set-at-a-time 集合代数
   - 只接受良好的 HTML, 不做高级处理

4. 设计原则
   1. bitmap 是临时的，index 是最终的
   2. 能用 prefix/scan 消解的依赖，不落到 per 字节分支
   3. 结构层先规范化再配对
   4. 解析结果只读


5. 目标不是“同一 DOM 上重复查询”的 warm-cache 数字，而是打破解析/选择里“看起来必须串行、必须分支”的依赖：
   - **解析**：块间只有常数个 carry（`in_tag`/`in_quote`），不随字节数增长；tokenizer 只访问标签候选，不逐字节分支；嵌套用规范化 + 先序编号，而不是每字节栈；
   - **选择**：祖先/兄弟用前缀计数、scatter、segmented scan 表达，不做 per-element 回溯；选择器编译成集合代数，而不是逐元素 right-to-left 试探；
   - **分支/发散**：branch misses 显著低于标量 tokenizer / 逐元素 matcher baseline；
   - **冷启动单遍**：parse + 首次 select 的吞吐/延迟（不缓存查询结果、不复用上次的 selector 结果）；
   - **批量**：docs/s 的强 scaling。


| 类别 | 内容 | 处理 |
|---|---|---|
| 支持 | `<!DOCTYPE>`、注释、普通开始/结束标签、空元素、常见可选结束标签、双/单/无引号属性、布尔属性、字符实体、UTF-8 | 正常解析 |
| 支持 | `script/style/textarea/title/xmp/iframe/noembed/noframes/plaintext` 的 rawtext 语义 | 掩码内容，保留标签 |
| 尽力 | 轻微错嵌套的内联标签、重复属性、未知标签、`<table>` 内少量不规范内容 | 尽量恢复，打 warning |
| 回退/拒绝 | adoption agency（`<b><i></b></i>`）、foster parenting、`<template>`、SVG/MathML、CDATA、非 UTF-8（先解码） | 标记 `needs_fallback`，由模式决定回退 html5ever 或 best-effort |
| 不做 | 浏览器级 DOM、CSS 层叠/计算样式、JS、表单提交、XML 命名空间完整语义 | 非目标 |

### 2.1 simdjson：stage-1 位图 + 依赖的位并行消解

看 `Refs/simdjson/src/generic/stage1/`：

- **64 字节块**：`simd8x64<uint8_t>`，AVX2 下两块 32 字节，AVX-512 下一块 64 字节。
- **无分支分类**：`json_character_block::classify`（`Refs/simdjson/src/haswell.cpp`）用 `vpshufb` 查表 + `cmpeq`，同时得到 `whitespace` 和 `op`（`, : [ ] { }`）两个 64 位位图。
- **转义/引号依赖**：
  - `json_escape_scanner` 处理反斜杠奇偶；
  - `json_string_scanner` 用 `prefix_xor(quote)` 一次算出“字符串内”掩码（`Refs/simdjson/src/generic/stage1/json_string_scanner.h`）；
  - `prefix_xor` 在 x86 上用 `_mm_clmulepi64_si128`（PCLMULQDQ）实现（`Refs/simdjson/include/simdjson/haswell/bitmask.h`）。
- **只访问候选**：stage-1 输出 structural index（每个结构字符的字节位置），stage-2 只按索引访问，不逐字节扫。
- **按需解析**：On-Demand API 连 tape 都不建，查询到哪解析到哪。

可迁移的：块扫描、位图、`prefix_xor`、flatten、只访问候选。
不可直接迁移的：JSON 的字符串/转义模型比 HTML 简单；HTML 没有“全局字符串”这个概念。

### 2.2 html5ever / parsel：语义与 API 的边界

- `html5ever` 的 tokenizer 有 ~40 个状态（`Refs/html5ever/html5ever/src/tokenizer/states.rs`），tree builder 有 ~20 个 insertion mode（`.../tree_builder/types.rs`）。真正复杂的是 **tree construction**，不是字节扫描：adoption agency、foster parenting、implied end tags、foreign content。
- `parsel` 的 API 很薄：`Selector` 包 lxml，CSS 经 `cssselect` 翻成 XPath，`::text`/`::attr()` 是扩展伪元素（`Refs/parsel/parsel/csstranslator.py`）。
- 这告诉我们：**API 兼容不难，难的是“选择结果和 lxml 一致”**；而一致性主要受 tree construction 影响。

| 问题 | 采用的做法 |
|---|---|
| 字节分类 | 64B 块 + `vpshufb`/`cmpeq` + 位图 |
| 引号/转义依赖 | `prefix_xor`（CLMUL） |
| HTML 特有的区域依赖 | `memmem`/Aho-Corasick 找 comment/rawtext 边界，先掩码 |
| 嵌套结构 | **先规范化成 Dyck 序列**，stack 建先序扁平文档 |
| 查询 | 扁平数组 + `subtree_end`；set-at-a-time 集合代数，逐元素只做 fallback（见 8.6） |
| 并行粒度 | 单文档内位并行；跨文档用 rayon 批量（M6） |

## 3. 核心映射：HTML 的依赖图

JSON 的解析依赖只有两类：escape 和 quote parity。HTML 多出两类：**区域依赖**和**树构造依赖**。把依赖拆清楚，才能决定哪些能位并行、哪些只能标量。

### 3.1 字节分类集合

Stage-1 只需要识别这些字节：

```
<  >  /  =  "  '  &  !  ?  -  whitespace(\t \n \r \f space)
```

- `<` / `>`：标签边界；
- `/`：结束标签或自闭合；
- `=`：属性赋值；
- `"` / `'`：属性值引号；
- `&`：实体；
- `!` / `?` / `-`：注释、DOCTYPE、PI；
- whitespace：分隔。

标签名、属性名的字符不需要逐字节分类；stage-2 在标签 span 内做标量扫描即可。

### 3.2 四类依赖

1. **引号依赖（tag 内）**：`>` 只有在“不在引号内”时才结束标签。引号只在 tag 内生效。
2. **标签开合依赖**：`in_tag` 在 `<` 处置位、在合法 `>` 处清零；它又依赖 `in_quote`，`in_quote` 又依赖 `in_tag`。这是一个 4 状态自动机（TEXT / TAG / DQ / SQ）。
3. **区域依赖**：`<!-- -->`、`<script>...</script>`、`<style>...</style>`、`<textarea>`、`<title>` 等区域内的 `<`/`>`/引号不应参与上面的自动机。区域边界用字符串搜索找，不是 parity 能解决的。
4. **嵌套依赖**：HTML 的标签不是简单括号：
   - 空元素（`br/img/input/...`）没有结束标签；
   - 可选结束标签（`p/li/td/tr/option/...`）经常省略；
   - 错嵌套需要 adoption agency。

### 3.3 依赖消解总表

| 依赖 | CPU SIMD | 备注 |
|---|---|---|
| UTF-8 | simdjson lookup4 或 `simdutf8` | ASCII 快速跳过 |
| 字节分类 | 64B 块 + `vpshufb`/`cmpeq` | `movemask` 拼 64 位位图 |
| 引号 parity | `prefix_xor`（CLMUL） | 按块带 carry |
| `in_tag`/`in_quote` | 位并行不动点（2–3 轮） | 不收敛时标量修复该块 |
| comment/rawtext | `memmem`/Aho-Corasick 找边界，掩码 | 区域数量少，串行链可接受 |
| 实体 | `&` 位图 + 懒解码 | 只在取文本/属性时解码 |
| 嵌套 | stack（O(n)） | 先规范化 |
| 可选结束标签 | 规则表 + scope 栈 | 规则约 10 条 |
| 选择匹配 | 扁平数组 + 集合代数 | 逐元素只做 fallback |

## 4. 总体架构

### 4.1 流水线

```
             bytes (Python buffer / mmap / network)
                        │
             ┌──────────▼──────────┐
             │ 0. 编码归一化        │  BOM / <meta charset> / encoding_rs
             └──────────┬──────────┘
                        │ UTF-8 bytes
             ┌──────────▼──────────┐
             │ 1. SIMD Scanner     │  64B block / CLMUL / tzcnt
             │  utf8 / 分类位图     │  rawtext 掩码 / in_tag 不动点
             └──────────┬──────────┘
                        │ 只保留 tag_starts/tag_ends
             ┌──────────▼──────────┐
             │ 2. Tokenizer        │  O(#tags) 标量
             └──────────┬──────────┘
             ┌──────────▼──────────┐
             │ 3. Normalizer       │  空元素/可选结束
             └──────────┬──────────┘
             ┌──────────▼──────────┐
             │ 4. FlatDoc          │  stack 建先序
             │  subtree_end        │  = pairing index
             └──────────┬──────────┘
             ┌──────────▼──────────┐
             │ 5. Selector         │  set-at-a-time 集合代数
             │  MatchRecord[]      │  逐元素 fallback
             └──────────┬──────────┘
             ┌──────────▼──────────┐
             │ 6. Materialize      │  host bytes + span
             └─────────────────────┘
```

关键点：

- 位图只在 stage-1 的块内存在，不按文档保存；最终只保存标签位置（约 1–5%）和节点数组；
- `MatchRecord` 在 v1 就已经存在（`::text`/`::attr` 需要），不是为 GPU 预留的。

### 4.2 crate 布局

```
wjsel/
├── Cargo.toml                # workspace
├── wjsel-core/               # 公共类型：NodeId, TagId, spans, MatchRecord, errors, flags
├── wjsel-simd/               # 64B block reader, classify, prefix_xor, bit indexer, utf8
├── wjsel-html/               # raw mask, tokenizer, normalizer, flat doc
├── wjsel-select/             # CSS/XPath parser, set-at-a-time matcher, 逐元素 fallback
├── wjsel-py/                 # PyO3 binding（cdylib）
├── wjsel/                    # facade：re-export + 高层 API
└── wjsel-bench/              # M9 的对比评测（开发期只放 microbench）
```

依赖建议：

- `memchr`（SIMD `memchr`/`memmem`）、`aho-corasick`（多模式 rawtext 搜索）；
- `simdutf8`（v1 UTF-8，后续可换自研 lookup4）；
- `cssparser`（CSS 语法，不直接用它做匹配）；
- `encoding_rs`、`entities`（HTML 实体表）、`smallvec`、`bitflags`、`rustc-hash`；
- `pyo3` + `maturin`；
- dev：`criterion`、`proptest`、`cargo-fuzz`、`arbitrary`。


补充约定：

- 文本节点的 `subtree_end = id + 1`（没有后代）；
- `subtree_end` 在节点闭合时写入；未闭合节点在 EOF 处闭合；
- `outer_end` 对元素是结束标签的末尾，对空元素是开始标签的末尾，对文本是文本末尾；
- `children`/`child_start` 是 set-at-a-time 查询需要的辅助数组，按需构建以省内存；`prev_element_sibling` 常驻；
- 没有 `Backend` 枚举、没有 dispatch trait、没有为别的后端预留的字段。

**先序 + `subtree_end` 是整个设计的支点**：

- 节点按先序编号，节点 `i` 的所有后代都在 `[i+1, subtree_end[i])` 连续区间内；
- `subtree_end` 与 cuJSON 的 pairing index 是同一类想法（记录配对的结束位置），只是这里由 stack 直接写出来；
- 查询时可以用区间做 `:has`、子树剪枝、以及“跳过不匹配子树”；
- 不需要指针树，缓存局部性好，且整个结构是连续数组。

---

## 5. Stage-1：Markup Index（SIMD 的核心）

### 5.1 块扫描器

- 固定 64 字节块；尾部用零填充，但记录有效长度，避免把填充当输入。
- AVX2：两个 32 字节 `__m256i`；AVX-512：一个 64 字节 `__m512i`；NEON：四个 16 字节。
- 每块输出（临时，不落盘）：

```
lt, gt, slash, eq, dq, sq, amp, ws : u64
raw                                : u64   // 1 = 在 comment/rawtext 内
in_tag, in_quote                   : u64
tag_starts, tag_ends               : u64   // 最终压平用
```

- 块间只带两个 carry：`prev_in_tag`、`prev_in_quote`。

### 5.2 分类器

- 可以为 12 个字符写 12 次 `cmpeq`，但更好的做法是 simdjson 式查表：
  - 低 nibble 查 16 项表，映射到候选字符；
  - 再用高 nibble/范围比较确认；
  - 对 `<`/`>`/`"`/`'`/`&` 这种高频字符，直接 `cmpeq` 也很便宜。
- whitespace 用范围比较：`c == ' ' || (c >= 0x09 && c <= 0x0D)`。
- 输出位图用 `movemask`（AVX2 返回 32 位，两块拼 64 位）。
- 目标：分类本身 ≥ 10 GB/s（AVX2），实际以 microbench 为准。

### 5.3 `in_tag` / `in_quote` 的位并行不动点

定义：位图第 `i` 位表示“处理完第 `i` 个字节后的状态”。`prev(x)` 表示左移一位并带入上一块的 carry。

```
raw = mask_raw(input)                 // 见 5.4

lt = eq('<') & ~raw
gt = eq('>') & ~raw
q  = (eq('"') | eq('\'')) & ~raw

// 只有 '<' 后面是 [A-Za-z] / '/' / '!' / '?' 才可能是标签。
// 这样文本里的 "1 < 2"、"a < b" 不会把后续全部吞进 tag。
// next_is_tag_start 用“右移一位”的位图得到（带下一块的 carry）。
next_is_tag_start = alpha | slash | bang | question
lt_open = lt & next_is_tag_start

// 初始：假设所有候选 '<' 都开标签，所有 '>' 都关标签（忽略引号）
t = prefix_xor(lt_open | gt, carry_t)

for _ in 0..3 {
    // 引号只在 tag 内生效
    q_new = prefix_xor(q & prev(t), carry_q)

    // 只有“不在引号内”的 '>' 才能关标签
    // 只有“不在 tag 内”的 '<' 才能开标签
    lt_ok = lt_open & ~prev(t)
    gt_ok = gt & prev(t) & ~prev(q_new)
    t_new = prefix_xor(lt_ok | gt_ok, carry_t)

    if (t_new, q_new) == (t, q) { break }
    t = t_new; q = q_new
}
```

- `lt_open` 的 lookahead 是必须的：HTML 文本里允许出现 `<`（例如 `1 < 2`），没有这个过滤，`<` 会把后面的文本全部当成 tag。
- 对合法 HTML，通常 1–2 轮收敛。
- 不收敛（说明有 `<` 在 tag 内、或引号错配）时：该块退化为“候选字节标量状态机”——只遍历 `lt_open|gt|q` 的置位位，逐位推进 4 状态机。候选数很少，代价可控。
- 注意位偏移：`>` 在它自身位置之后才关标签，`<` 在自身位置之后才开；实现里用 simdjson 的 `follows()` 风格 `(mask << 1) | carry`，不要直接混用“字节位”和“状态位”。

`prefix_xor` 的实现：

```rust
#[inline]
fn prefix_xor(mut x: u64) -> u64 {
    // x86: _mm_clmulepi64_si128(x, 0xFFFF...FF, 0)
    // 通用: 5 次 shift + xor
    x ^= x << 1;
    x ^= x << 2;
    x ^= x << 4;
    x ^= x << 8;
    x ^= x << 16;
    x ^= x << 32;
    x
}
```

带 carry 的版本：`if carry { !prefix_xor(x) } else { prefix_xor(x) }`。

### 5.4 comment / rawtext 的区域掩码

区域依赖不能靠 parity 解决，必须显式找边界。策略是“两遍 stage-1”：

**Pass A：找区域**

1. `<!--` … `-->`：`memmem` 找 `<!--`，再找 `-->`；找不到就到 EOF。整个区间（含分隔符）标记 raw。
2. `<!DOCTYPE` / `<?...>`：从 `<!` 或 `<?` 到下一个 `>`，标记 raw（不建节点）。
3. rawtext 开始标签：在 Pass A 的近似标签流里，找名字属于
   `script/style/textarea/title/xmp/iframe/noembed/noframes/plaintext`
   的开始标签；然后大小写不敏感地找 `</name`，再找到它的 `>`。
   - 区间 `(开始标签的 '>' , 结束标签的 '<')` 标记 raw；结束标签本身保留。
   - 对 `<script>` 内出现 `</script>` 字符串的情况，不实现 spec 的 double-escape 状态；按第一次出现处理（这是常见浏览器行为，也是“允许不处理怪异 HTML”的一部分）。
4. 合并/排序区间，处理重叠。

**Pass B：带 raw 掩码重跑 5.3**

- raw 内的 `<`/`>`/引号全部清零；
- raw 跨块时，直接把整块清零，块边界做部分掩码。

性能：区域数量通常很少（一个页面几个到几十个），`memmem` 是 SIMD 的；Pass A 只对标签候选做名字检查，代价 O(#tags)，不是 O(bytes)。

### 5.5 UTF-8

- v1：用 `simdutf8`（成熟、快、无 unsafe 暴露）。
- 后续（如果要融合）：把 simdjson 的 `utf8_lookup4_algorithm.h` 移植过来，与分类在同一个 64B 块循环里做，避免二次扫。
- HTML 编码：先看 BOM，再在前 1024 字节找 `<meta charset=...>` / `http-equiv`；非 UTF-8 用 `encoding_rs` 解码成 UTF-8 再进 stage-1（会多一次拷贝，但保证后续都是 UTF-8）。

### 5.6 flatten：只保留标签候选

```rust
let mut bits = tag_starts;
while bits != 0 {
    let i = bits.trailing_zeros() as u32;
    tag_starts.push(block_start + i);
    bits &= bits - 1;
}
```

- 标签开始：`lt_ok` 的置位位；
- 标签结束：`gt_ok` 的置位位；
- 文本区间：`tag_ends[k]+1 .. tag_starts[k+1]`；
- 不保存 `lt/gt/q/t` 位图；
- 如果 `tag_starts.len() != tag_ends.len()`，标记 `UNBALANCED_TAG`，交给 tokenizer 做恢复或 fallback。

### 5.7 和 simdjson 的差异

| 维度 | simdjson | wjsel stage-1 |
|---|---|---|
| 结构字符 | 6 个固定字符 | 12 个字符 + raw 区域 |
| 字符串 | 全局 quote parity | tag 内 quote parity，受 `in_tag` 门控 |
| 区域 | 无 | comment/rawtext 需要显式掩码 |
| 输出 | structural index | tag_starts/tag_ends |
| 失败 | 严格报错 | 标记 + 标量修复 + 可选 fallback |

---

## 6. Stage-2：Tokenizer（标量，O(#tags)）

### 6.1 标签解析

对每个 `[tag_start, tag_end]`：

```
'<' [ '/' ] name (ws attr)* [ '/' ] '>'
```

- 名字：ASCII 字母开头，后续取到 whitespace / `/` / `>`；大小写不敏感（ASCII lower）。
- 结束标签：`</name>`，忽略属性。
- 开始标签：循环读属性：
  - 跳过 whitespace；
  - 读属性名到 whitespace / `=` / `>` / `/`；
  - 若 `=`，跳过空白，读值：
    - `"..."` / `'...'`：到匹配引号；
    - 无引号：到 whitespace / `>`；
  - 布尔属性：没有 `=`；
  - 重复属性：第一个生效（HTML 规范）；
  - 属性名 lower。
- 自闭合：`/` 在 `>` 前；对空元素等价，对非空元素在 HTML 里忽略（v1 按“忽略”处理，foreign content 后续再说）。
- 属性值保留原始 span；`&` 在取用时解码。

### 6.2 实体

- stage-1 的 `amp` 位图告诉我们哪些文本/属性值含 `&`。
- v1 用 `entities` crate 的完整 HTML 命名实体表 + 数字实体；支持 `&amp;` `&#38;` `&#x26;`，以及常见省略分号的 legacy 形式（可选）。
- 解码是**惰性**的：只有 `::text`、`.text`、`::attr()`、`:contains` 才解码；纯结构查询不解码。
- 解码结果按节点缓存（`OnceCell`/`Mutex` 可选），避免重复。

### 6.3 空元素与可选结束标签

**空元素**（无结束标签）：

```
area base br col embed hr img input link meta param source track wbr
```

Normalizer 会为每个空元素补一个**紧跟其后的合成 `End`**，这样事件流始终是 Dyck 序列；查询侧看到的就是 `subtree_end = id + 1`。

**可选结束标签规则**（简化版，按 scope 栈）：

| 遇到 | 关闭 |
|---|---|
| `li` | 当前 `ul/ol` scope 内打开的 `li` |
| `dt` / `dd` | 当前 `dl` scope 内打开的 `dt/dd` |
| `p` | 打开的 `p` |
| 块级开始标签 | 打开的 `p` |
| `option` | 打开的 `option` |
| `optgroup` | `option` / `optgroup` |
| `tr` | `td/th` / `tr` |
| `td` / `th` | 打开的 `td/th` |
| `thead/tbody/tfoot` | `td/th/tr/thead/tbody/tfoot` |
| `colgroup` | `colgroup` |
| `rt` / `rp` | `rt/rp` |

块级标签集合（用于关 `p`）：`address article aside blockquote div dl fieldset footer form h1-h6 header hr main nav ol p pre section table ul`。

实现：Normalizer 维护一个栈，栈元素带 `scope` 标记（`ul/ol/dl/table/select/ruby`）。遇到开始标签时，先按规则弹栈到 scope 边界；弹不动的（跨 scope）就保留并打 warning。

### 6.4 输出

```rust
enum Event {
    Start { tag: TagId, attrs: Range<u32>, self_closing: bool, span: Range<u32> },
    End { tag: TagId, span: Range<u32> },
    Text { span: Range<u32>, has_entity: bool, raw: bool },
}
```

- 注释/doctype 不进入事件流（stage-1 已 mask）。
- 文本节点合并相邻区间。
- `raw = true` 的文本（`script/style/textarea/title` 内容）保留为文本节点，但不解析标签、不解码实体；`script::text` 仍可被选择。
- 事件流是 Normalizer 的输入，也是 FlatDoc builder 的输入。

---

## 7. Stage-3：扁平文档

### 7.1 为什么不建指针树

- `Rc<RefCell<Node>>` 每个节点一次分配、缓存差、Python 侧也难共享；
- 查询只需要：父、子、兄弟、子树范围、深度、sibling index；
- 先序数组 + `subtree_end` 能表达全部这些，且是连续内存；
- 先序编号 + 连续子树区间，是后面 set-at-a-time 集合代数能成立的前提（见 8.6）。

### 7.2 stack + 先序编号

```rust
let mut stack: Vec<NodeId> = vec![root];
for ev in events {
    match ev {
        Start { .. } => {
            let parent = *stack.last().unwrap();
            let id = nodes.len() as NodeId;
            nodes.push(Node::new_element(parent, ...));
            link_child(parent, id);
            stack.push(id);
        }
        End { tag, span } => {
            // 找到最近的同名开始标签；中间不匹配的节点按“弹到匹配”恢复
            let mut found = None;
            for i in (1..stack.len()).rev() {
                if nodes[stack[i]].tag == tag { found = Some(i); break }
            }
            if let Some(i) = found {
                // i 之上的节点全部闭合
                while stack.len() > i + 1 {
                    let popped = stack.pop().unwrap();
                    nodes[popped].subtree_end = nodes.len() as NodeId;
                    nodes[popped].outer_end = span.start;
                }
                let popped = stack.pop().unwrap();
                nodes[popped].outer_end = span.end;
                nodes[popped].subtree_end = nodes.len() as NodeId;
            } else {
                // 没有同名开始标签：忽略这个结束标签
                flags |= MISNESTED;
            }
        }
        Text { span, .. } => {
            let parent = *stack.last().unwrap();
            let id = nodes.len() as NodeId;
            nodes.push(Node::new_text(parent, span));
            link_child(parent, id);
            nodes[id].subtree_end = id + 1;
            nodes[id].outer_end = span.end;
        }
    }
}
// 收尾：未闭合的节点在 EOF 处闭合
while stack.len() > 1 {
    let popped = stack.pop().unwrap();
    nodes[popped].subtree_end = nodes.len() as NodeId;
    nodes[popped].outer_end = input.len() as u32;
}
```

- `link_child` 维护 `first_child` / `next_sibling` / `prev_element_sibling`；`sibling_index` 和 `type_index` **只对 Element 计数**（文本节点不占 `:nth-child` 的位置）。
- `outer_end`：元素自己的 `>` 或最后一个后代的末尾；`get()` 用它切原始字节。
- 栈深度上限（默认 1024，可配置）防止恶意深嵌套。

### 7.3 校验与回退

- `MISNESTED` / `UNBALANCED_TAG` / `DEPTH_LIMIT` → `DocFlags`。
- `ParseMode`：
  - `Fast`：best-effort，保留已解析部分，warnings 暴露；
  - `Safe`（默认建议）：检测到 flag 时整篇回退 html5ever（feature `spec-fallback`）；
  - `Strict`：直接报错。
- 回退成本：一篇文档重解析；批量时只影响少数文档，可接受。

### 7.4 查询需要的索引

| 查询需求 | 索引 |
|---|---|
| 后代/子 | `parent`, `first_child`, `next_sibling`, `subtree_end`；集合代数另用 `children`/`child_start` |
| 兄弟 | `next_sibling`, `prev_element_sibling`, `sibling_index` |
| `+` / `~` | `prev_element_sibling`；`~` 在集合代数里用 `children`/`child_start` 做 segmented scan |
| `:nth-child` / `:first-child` | `sibling_index` |
| `:nth-of-type` / `:first-of-type` | `type_index`；`:last-of-type` 沿 `next_sibling` 同 tag 查找 |
| `:has` / 子树剪枝 | `subtree_end` |
| `get()` | `span`, `outer_end`, `input` |
| `::attr` | `AttrSpan.value` |
| `::text` | 文本节点 + 实体解码 |

---

## 8. Stage-4：选择器

### 8.0 先判断：Stylo 的算法能不能并行？

`Refs/stylo/selectors/matching.rs` 的做法是**逐元素、从右到左、带回溯**：

- 对每个元素调用 `matches_complex_selector_internal`；
- 组合器用 `next_element_for_combinator` 走 `parent_element` / `prev_sibling_element`；
- 用 ancestor Bloom filter（`bloom.rs`，4096 个计数器、2 个 hash）快速拒绝；
- `:nth-child` 用 `NthIndexCache`（`nth_index_cache.rs`）；
- `:has` 递归搜子树/兄弟（`matches_relative_selector`）。

它的问题不是“在 CPU 上慢”，而是**依赖形态**：每个元素的组合器都是一次 parent/prev 链跳转，最坏要回溯；这正是“看起来必须串行、必须分支”的那部分。逐元素实现天然带分支和回溯，规模上去后代价随数据分布波动。

可以换一种形式：把“逐元素匹配”改成“**集合到集合**”的位图代数（set-at-a-time）：所有组合器都变成前缀计数 / scatter / segmented scan，不再有 per-element 回溯。这是本节 8.6，也是 v1 selector 的主路径。

### 8.1 CSS 子集

v1 支持：

- 类型、`*`、`.class`、`#id`；
- `[attr]`、`[attr=v]`、`[attr^=v]`、`[attr$=v]`、`[attr*=v]`、`[attr~=v]`、`[attr|=v]`，以及 `i` 大小写不敏感标志；
- 组合器：后代 ` `、子 `>`、相邻 `+`、一般兄弟 `~`；
- 伪类：`:first-child`、`:last-child`、`:only-child`、`:nth-child()`、`:nth-last-child()`、`:first-of-type`、`:last-of-type`、`:nth-of-type()`、`:not()`、`:is()`、`:where()`、`:root`、`:empty`；
- 伪元素：`::text`、`::attr(name)`（parsel 扩展）；
- `:has()` 放 v2（需要子树扫描，先保证正确性）；
- `:contains()` 非标准，v2 可选。

### 8.2 CPU 参考/回退：Stylo 风格逐元素匹配

把选择器编译成从右到左的指令序列：

```rust
struct Compound {
    tag: Option<TagId>,
    id: Option<Range<u32>>,
    classes: SmallVec<[Range<u32>; 4]>,
    attrs: SmallVec<[AttrPred; 4]>,
    pseudos: SmallVec<[Pseudo; 4]>,
}

struct Complex {
    compounds: Vec<Compound>,     // 从左到右
    combinators: Vec<Combinator>, // len = compounds.len() - 1
}
```

匹配：

1. 选“种子 compound”：优先级 `#id > .class > tag > [attr] > *`，估算候选数最小；
2. 遍历候选节点（全文档或某个子树区间）；
3. 从右到左验证 compound，遇到组合器：
   - `>`：`parent` 一步；
   - ` `：沿 `parent` 链上溯；
   - `+`：`prev_element_sibling`；
   - `~`：沿 `prev_element_sibling` 链上溯；
4. 全部通过则加入结果。

优化：

- `:nth-child` 用 `sibling_index`，O(1)；
- `:empty` 检查 `first_child == NONE`；
- `:not()` 递归匹配，但限制嵌套深度；
- 可选：ancestor Bloom filter（Stylo 的 4096-counter 版，`bloom.rs`）用于逐元素快速拒绝；v1 可以先不做，等 profile 证明值得；
- 只缓存**编译后的 selector AST/plan**（跨查询复用），不缓存匹配结果；不把“同一 DOM 重复查询”当作优化目标。

**这是参考实现和 fallback，不是 v1 的主路径。** 它保留了 per-element 的 parent/prev 跳转和回溯，恰恰是我们要打破的东西；v1 的主路径是 8.3/8.6 的 set-at-a-time，逐元素版用于：不支持的 selector 子集、`::text`/`::attr` 的边界情况、以及作为差分测试的 baseline。

### 8.3 CPU 主路径：set-at-a-time（v1）

Scrapy 常见模式：同一页面上跑几十个 CSS/XPath，且每个页面只解析一次。主路径不做“每个元素问一遍”，而是：

- 把 selector 编译成 `Complex`；
- 对每个 compound 求 `M(compound)` bitset；
- 用组合器做 bitset 传播：后代 = 前缀计数 + 区间，子/相邻/一般兄弟 = scatter / segmented scan；
- 最后 `M(S)` 升序展开成结果。

这是 **v1 的默认 selector 引擎**（8.6 的 CPU 实现）。逐元素 matcher（8.2）保留为 fallback 和差分 baseline。

CPU 上用 `u64` bitset + `memchr`/手写位运算；如果某个 compound 高度 selective（`#id`），可以先用 id/class/tag 倒排把候选集合缩小，再进入集合代数——**优化的是种子集合，不是把组合器退回逐元素回溯**。

### 8.4 XPath 子集

parsel 的用户大量用 XPath。v1 做一个够用的子集：

- 位置路径：`/`、`//`、`*`、`name`、`@attr`、`text()`；
- 谓词：`[n]`、`[@a]`、`[@a='v']`、`[contains(@a,'v')]`、`[starts-with(@a,'v')]`、`and`/`or`；
- 函数：`normalize-space()`、`string()`（可选）；
- 不支持：完整轴、命名空间、XPath 2.0/3.0、变量（v2 再说）。

实现方式：自己写一个小 parser（或复用 `xpathparser` 这类 crate 的 AST），编译到和 CSS 相同的 `Complex`/matcher IR。这样 CSS 和 XPath 共享匹配器。

XPath 先复用同一套集合代数覆盖 child/descendant/attribute/谓词子集；覆盖不到的部分回退 8.2。

绝对路径 `/...` 在嵌套 Selector 上的行为：parsel 会警告；v1 按“从文档根开始”处理，并打 warning。

### 8.5 结果物化

- **元素 `.get()`**：直接返回 `input[span.start .. outer_end]` 的切片。因为我们不改 DOM，这就是原始 outer HTML，**零拷贝**，比 lxml 重新序列化快。
  - 如果可选结束标签是合成的，`outer_end` 取“最后一个后代的末尾”，返回的片段仍然是合法的原始 HTML。
- **`::text`**：返回文本节点（逐个），实体解码；parsel 的 `::text` 是逐个文本节点，不是 `text_content()`。
- **`::attr(x)`**：返回属性值，实体解码。
- **`.text`**：拼接所有后代文本（类似 lxml `text_content()`）。
- **`.attrib`**：返回属性字典。
- **`.re()`**：对 `get()` 的结果跑 `regex`；`replace_entities` 参数保留。

兼容性注意：`.get()` 返回**原始 HTML 切片**，而 lxml 会重新序列化（可能改引号、补全实体、调整大小写）。结构等价但字符串可能不同；如果用户做逐字节比较，需要在评测/测试里做规范化。这是“零拷贝 + 保留原始格式”换来的差异，设计上认为值得。

### 8.6 set-at-a-time 位图代数（v1 主路径）

把匹配从“逐元素”改成“**集合到集合**”。设 `M(S)` 是匹配选择器 `S` 的节点 bitset（`N` 位，`N/64` 个 `u64`），`N` 是节点数。所有运算都是 bitset 上的 AND/OR/NOT、前缀计数、segmented scan、scatter。`M(S)` 默认只覆盖 **Element** 节点；文本节点只在 `::text` 投影时出现。

实现：`u64` bitset + 手写位运算/前缀。

| CSS | 集合表达式 | 实现原语 |
|---|---|---|
| 复合选择器（tag/class/id/attr/pseudo） | 交集 | 逐节点谓词 + AND |
| `A B` | `A ∩ ancestors(B)` | 前缀计数 + 区间判定 |
| `A > B` | `A ∩ parents(B)` | scatter（`|=`） |
| `A + B` | `A ∩ prev_element_sibling(B)` | scatter |
| `A ~ B` | `A ∩ earlier_element_siblings(B)` | child-group 上的反向 segmented OR |
| `:not(S)` | `~M(S)` | bitset NOT |
| `:is(S1, S2)` / `:where` | `M(S1) ∪ M(S2)` | bitset OR |
| `:has(> B)` | `parents(M(B))` | scatter |
| `:has(B)` | `ancestors(M(B))` | 前缀计数 |
| `:has(+ B)` | `prev_element_sibling(M(B))` | scatter |
| `:has(~ B)` | `earlier_element_siblings(M(B))` | segmented scan |
| `:nth-child(An+B)` | `sibling_index ∈ An+B` | 逐节点比较 |
| `:nth-last-child(An+B)` | `child_start` 的组长度 - `sibling_index` + 1 | 逐节点比较 |
| `:nth-of-type` | `type_index` | 逐节点比较 |
| `:empty` / `:root` / `:scope` | 局部字段 | 逐节点比较 |
| `::text` / `::attr` | 匹配后的投影 | gather / 逐节点 |

`:nth-last-of-type` 需要 type 计数，v2 再做；`:nth-child(An+B of S)` 需要按 sibling group 过滤后再做 segmented prefix，v1 回退逐元素。

两个关键公式：

**后代 `ancestors(B)`**。利用先序编号 + `subtree_end`：

```
P[i] = |B ∩ [0, i)|                 // B 的前缀计数
ancestors(B) = { i | P[subtree_end[i]] - P[i+1] > 0 }
```

节点 `i` 的后代是连续区间 `[i+1, subtree_end[i])`；区间内 B 的数量就是两个前缀计数之差。**O(N)，不需要沿 parent 链上溯。** 这是整个集合代数最漂亮的一步。

**一般兄弟 `earlier_element_siblings(B)`**。对每个 parent 的 child group（`children` + `child_start`），做一次反向 segmented suffix OR：某个 child 的后面只要出现过 B，就置位。`~` 和 `:has(~ B)` 都靠它。

复杂选择器的求值顺序是从右到左：

```
S = M(Ck)
for i = k-1 .. 1:
    S = M(Ci) ∩ op(combinator_i, S)
```

其中 `op` 是 `ancestors` / `parents` / `prev_element_sibling` / `earlier_element_siblings`。

成本：

- 每个 compound 一次 O(N) 谓词扫描；每个 combinator 一次 O(N)（前缀/segmented）或 O(|S|)（scatter）；
- bitset 本身只有 `N/64` 个字，AND/OR/NOT 是纯带宽操作；
- 内存只需 3–4 个活跃 bitset（`N/8` 字节每个）+ 一个 prefix 数组；
- 结果按 NodeId 升序扫描，天然是文档顺序，和 `querySelectorAll`/parsel 一致。

不支持的子集（回退 8.2 逐元素）：

- `:nth-child(An+B of S)` / `:nth-last-child(... of S)`：需要按 sibling group 过滤后再做 segmented prefix；
- 嵌套 `:has`：递归集合代数，设深度上限；超过就回退；
- `:hover`/`:focus`/`:visited`/`:target` 等动态伪类：静态文档里不匹配，返回空；
- `:lang()`/`:dir()`：v2。

一个前提：属性/class/id 的匹配值必须是**解码后的值**（HTML 实体）。CPU 上按需解码，或对 selector 引用到的属性做一次解码。

### 8.7 parsel 兼容：返回结果，不返回内部结构

选择结果不是裸 NodeId，而是一条很小的 `MatchRecord`（见 4.3）：

- `Selector` 持有 `Arc<FlatDoc>` + `MatchRecord`；`SelectorList` 持有 `Arc<FlatDoc>` + `Vec<MatchRecord>`；
- `.css()` 在已有结果上继续查询：把当前结果的子树并集作为 root mask，与新的 `M(S)` 相交，再跑一遍集合代数；
- `.get()` 直接切 host bytes 的 `span/outer_end`；`::attr` 用 `attr` span；`::text` 用文本节点的 span；`.text`/`.attrib` 需要时只重解析结果节点的一小段；
- `.xpath()` 与 CSS 共享 `Complex` 与集合代数；不支持的部分回退 8.2。

这是 v1 的形态，不需要任何后端抽象。

### 8.8 决策

**结论：集合代数是 v1 selector 的主路径。**

- **不是**把 Stylo 的逐元素 matcher 当主路径；它是 fallback、边界情况和差分 baseline。
- M4 直接实现 CPU set-at-a-time；M4.5 只做调优和与逐元素版的差分。
- 如果某个 selector 子集无法用集合代数表达，就回退 8.2 的逐元素 matcher；这不影响 v1 的完整性。

一句话：**Stylo 的算法是“每个元素问一遍”，集合代数是“整个集合算一遍”；后者在 CPU 上就打破 per-element 回溯。**

---

## 9. Python binding

### 10.1 API 映射

| parsel | wjsel |
|---|---|
| `Selector(text=..., type="html")` | 同名 |
| `Selector(body=bytes, encoding=...)` | 同名 |
| `.css(query)` | 同名，返回 `SelectorList` |
| `.xpath(query, namespaces=...)` | 同名（子集） |
| `.get()`, `.getall()` | 同名 |
| `.attrib`, `.text` | 同名 |
| `.re()`, `.re_first()` | 同名 |
| `SelectorList` 的索引/迭代/`len`/`bool` | 同名 |
| `.css()` 链式 | 同名（在子树内匹配） |
| `type="xml"` | v1 抛 `NotImplementedError` |
| `base_url` | 保存，不联网 |
| 可选批量 | `parse_many(list[bytes]) -> list[Selector]`；只是 rayon 并行调用同一套 CPU 流程（M6），语义与逐个 `Selector(...)` 相同 |

### 10.2 所有权、GIL、零拷贝

- `Selector` 持有 `Arc<FlatDoc>` + `MatchRecord`；`SelectorList` 持有 `Arc<FlatDoc>` + `Vec<MatchRecord>`。没有后端枚举，没有 dispatch。
- 解析和匹配用 `py.allow_threads(|| ...)` 释放 GIL；物化时再拿 GIL。
- 输入：
  - `str`：编码成 UTF-8，拷贝一次；
  - `bytes`：v1 拷贝一次（简单安全）；后续可用 `PyBuffer` + 持有 `Py<PyBytes>` 做真零拷贝。
- 输出：
  - `get()` 对元素返回 `input[start..outer_end]` 的 `&str`（UTF-8 已保证）；对 `::attr` 返回属性值；对 `::text` 返回文本节点内容；
  - `getall()` 在释放 GIL 的闭包里收集 `Vec<String>`，最后一次性建 `PyList`。
- 异常：解析不抛异常（除非 `Strict`）；非法选择器抛 `ValueError`；panic 由 PyO3 转成 `PanicException`，不让进程崩。

### 10.3 打包

- 生成用 `maturin`，`abi3-py39`，先出 Linux x86_64 wheel；
- 版本策略：`0.x` 期间 API 可能微调，但 parsel 兼容层保持稳定。

---

## 10. 正确性、回退与安全

### 11.1 三级模式

| 模式 | 行为 | 适用 |
|---|---|---|
| `Fast` | 只跑受限解析；flag 只记录，不修复 | 爬虫，能接受少量偏差 |
| `Safe` | flag 触发整篇 html5ever 回退 | parsel 替代，默认建议 |
| `Strict` | flag 直接报错 | 测试/数据管线 |

回退实现：feature `spec-fallback` 依赖 `html5ever`，把它的 rcdom 转成我们的 `FlatDoc`。回退路径只在少数文档上走，性能可接受。

### 11.2 差分测试

- **tokenizer/tree construction**：用 `Refs/html5ever/rcdom/custom-html5lib-*-tests` 和 `wpt-tree-construction-tests`，把期望 DOM 规范化后与 `FlatDoc` 比较；只统计“in-scope”用例，out-of-scope 计入 fallback 率。
- **选择器**：构造查询语料（CSS + XPath），在合法 HTML 上与 parsel/lxml 比较 `getall()`；允许顺序差异时按集合比较。
- **selector 引擎内部**：同一批 selector/文档，set-at-a-time 与 8.2 逐元素 matcher 的结果集/顺序必须一致；不支持的子集记录 fallback 率。
- **模糊测试**：`cargo-fuzz` 覆盖 stage-1、tokenizer、CSS/XPath parser；目标是“不 panic、不越界、内存有界”。
- **sanitizer**：nightly `-Z sanitizer=address` 跑测试；`miri` 只覆盖不碰 intrinsics 的纯 Rust 部分。
- GPU 阶段的差分（CPU vs GPU 的 `MarkupIndex`/`FlatDoc`/`MatchRecord`）见第 13 节。

### 11.3 安全

- 不做 DTD/外部实体，不做网络请求；
- 实体表是静态的，解码有长度上限；
- 栈深度、节点数、属性数、选择器嵌套深度都有上限；
- 输入长度用 `usize` 检查，所有 `u32` span 在超过 4 GiB 时拒绝（或切 64 位）；
- `unsafe` 只允许出现在 `wjsel-simd` 的边界，集中审计；
- Python 侧不暴露裸指针。

---

## 11. 性能模型与最终评测（做好之后）

### 12.1 成本模型

```text
T_cpu ≈ B / R_scan + N_tags * C_tag + k * (N/64) * C_bitset + N * C_prefix
```

- selector 的主路径是 bitset 集合代数：`k` 个 compound/combinator，每个 ~`N/64` 个 word 操作；`ancestors` 还多一次 O(N) 前缀。逐元素 fallback 才用 `N_candidates * C_match`。
- `R_scan`：CPU 分类+位图吞吐（AVX2 目标 5–15 GB/s，实测为准）；
- `C_tag`：单标签 tokenize 成本；
- `C_bitset`：单 word 的 bitset 操作成本；
- `C_prefix`：一次 O(N) 前缀/segmented 操作的成本。
- GPU 阶段的成本模型在第 13 节。

### 12.2 指标

**首要不是“同一 DOM 重复查询”的 warm 数字，而是依赖/分支指标：**

- **解析依赖**：每 64B block 的串行 carry 数（目标：常数 2 个状态位，不随字节增长）；tokenizer 访问的字节数 / 总字节数（目标 O(#tags)，不是 O(B)）。
- **选择依赖**：每个 combinator 的 per-element parent/prev 跳转次数（目标 0）；回溯次数（目标 0）；bitset 集合操作的次数。
- **分支**：branch misses / KB、IPC；与标量 tokenizer / 逐元素 Stylo baseline 对比。
- **冷启动单遍**：`parse + 首次 select` 的 ms / MB/s；每次迭代重新 parse、重新 query，不复用匹配结果。
- **批量**：docs/s vs batch size；强 scaling 效率。
- **资源**：峰值 RSS（子进程隔离，`ru_maxrss`）。
- **正确性**：与 lxml/parsel 的匹配数/结果集对比；`needs_fallback` 文档占比。
- **warm repeated-query 只作为附录**：可以报告，但不作为目标或验收线，避免用缓存命中掩盖依赖/分支问题。

### 12.3 语料

- 合成页面：8 KB / 64 KB / 512 KB / 4 MB / 16 MB，覆盖卡片、表格、列表、属性、实体、script/style；
- 真实页面：Scrapy/parsel 文档、simdjson 报告、servo.org、Shakespeare；
- 规范测试：html5lib tokenizer/tree-construction；
- 畸形页面：错嵌套、未闭合、超深嵌套、超大文本节点。

### 12.4 对比对象

- `lxml.html`（libxml2）；
- `parsel`（lxml + cssselect）；
- `selectolax`（lexbor，C）；
- `bs4 + lxml`、`bs4 + html5lib`（参考，不作为主要对手）。
- **内部 baseline**：标量 tokenizer、Stylo 风格逐元素 matcher；用于依赖/分支指标对比，不参与“谁更快”的排名。

方法：CPU cache 预热只用于稳定频率；**每次 measured iteration 重新 parse、重新执行 query，不复用匹配结果/selector 状态**；固定 CPU 频率、GC 关闭、取中位数；parse 与 select 分开测，同时报告冷启动单遍和批量。**按你的要求，这一步放在实现完成后的 M9，不在设计阶段跑。**

### 12.5 验收线（暂定）

| 项 | 目标 |
|---|---|
| CPU 解析（冷） | ≥ 3× lxml；≥ 1.2× selectolax（≥64 KB） |
| 冷启动单遍 | `parse + 首次 select` ≥ 2× parsel；不复用结果/selector 状态 |
| 选择依赖 | 每个 combinator 0 次 per-element parent/prev 回溯；祖先/兄弟用前缀计数 / scatter / segmented scan |
| 分支 | 报告 branch misses、IPC，与标量 tokenizer / 逐元素 Stylo baseline 对比；不设拍脑袋的绝对值 |
| 内存 | ≤ 2× 输入（典型页面） |
| 正确性 | in-scope 语料 100%；fallback < 1% |
| 批量 scaling | docs/s 近线性（rayon，M6） |
| warm repeated-query | 只放附录，不作为验收线 |

GPU 阶段的验收线见 13.8。

---

## 12. 先后次第（Roadmap）

### 12.1 里程碑总表（v1，纯 CPU）

| # | 里程碑 | 周期 | 交付 | 退出条件 |
|---|---|---|---|---|
| M0 | 设计冻结 | 1w | 本文档 + ADR + API 草案 | 数据布局/口径/回退策略拍板 |
| M1 | SIMD 基座 | 2w | `wjsel-simd`：block reader、classify、prefix_xor、bit indexer、utf8 | 与标量参考逐位一致；分类 ≥ 10 GB/s（AVX2） |
| M2 | Stage-1 | 3w | `MarkupIndex`：raw 掩码、不动点、tag 提取 | 合法语料 tag 流正确；不收敛有标量修复 |
| M3 | Tokenizer + FlatDoc | 3w | 属性/实体/空元素/可选结束；先序节点、parent、`subtree_end` | 与 html5ever 在 in-scope 用例上一致；512 KB 合成页 ≥ 3× lxml |
| M4 | Selector（set-at-a-time 主路径） | 4w | CSS 子集 + XPath 子集 + 集合代数 matcher + 逐元素 fallback + 物化 | 与 parsel 结果集一致；冷查询（首次、无结果缓存）≥ 2×；依赖/分支指标有基线 |
| M4.5 | Selector 调优/差分 | 1w | bitset 调优、与逐元素 matcher 差分、seed 索引（id/class/tag） | 同一语料结果集完全一致；性能对照有结论 |
| M5 | Python binding | 2w | PyO3/maturin、parsel 兼容层、wheel | 一个 Scrapy spider 子集可直接替换 |
| M6 | 硬化/调优 | 2w | fuzz、sanitizer、内存、rayon 批量、小文档快路径 | 无 panic/越界；内存 ≤ 2×（典型） |
| M9 | 对比评测 | 2w | 评测 harness + 报告 | 指标达标；公开方法与数据 |

v1 合计约 20 人周。**GPU 阶段（第 13 节）不进入 v1 排期**，在 M9 结论出来后再单独立项。

### 12.2 每个里程碑具体做什么

**M0：设计冻结**

- 确认 `FlatDoc` 布局、`NodeId` 先序、`subtree_end` 语义；
- 确认 v1 支持/回退清单；
- 确认 Python API 兼容范围（CSS 子集、XPath 子集）；
- 写 ADR：为什么不用 `Rc<RefCell>`、为什么 selector 主路径用 set-at-a-time 而逐元素只做 fallback、为什么 v1 不建 GPU 抽象、为什么需要 Normalizer。

**M1：SIMD 基座**

- 64B block reader（AVX2/AVX-512/NEON/scalar）；
- `classify`：12 个分隔符 + whitespace；
- `prefix_xor`（CLMUL + 通用）；
- `bit_indexer`：`tzcnt` flatten；
- SIMD 用 `std::arch` intrinsics + `#[target_feature]` + `is_x86_feature_detected!` 运行时派发；不依赖 nightly portable SIMD（若要跨架构抽象，可包一层 `pulp`）；
- UTF-8：先接 `simdutf8`，留自研接口；
- property test：随机字节 vs 标量参考；
- microbench：分类、prefix_xor、flatten。

**M2：Stage-1**

- raw 掩码（comment/doctype/rawtext，两遍）；
- `in_tag`/`in_quote` 不动点 + 标量修复；
- tag_starts/ends 提取、文本区间；
- 与 html5ever tokenizer 的差分（只比标签边界/名字/属性 span）；
- fuzz：随机 HTML 不 panic。

**M3：Tokenizer + FlatDoc**

- 属性解析、实体懒解码；
- 空元素、可选结束规则、Normalizer；
- 先序节点、parent/children/sibling、`subtree_end`；
- 与 html5ever tree builder 的差分（in-scope）；
- `ParseMode` 三档 + fallback 骨架。

**M4：Selector（CPU set-at-a-time 主路径）**

- CSS parser（`cssparser`）→ `Complex`；
- **主路径**：8.6 的 `M(S)` 集合代数——`ancestors` 用前缀计数，`parents`/`prev` 用 scatter，`~` 用 child-group 后缀 OR；
- **fallback**：8.2 的 Stylo 风格逐元素 matcher，用于不支持的子集/边界情况/差分；
- `::text`/`::attr`、`get/getall/attrib/text/re`；
- XPath 子集 parser；
- 与 parsel 的查询语料对比；冷查询、依赖/分支指标打点。

**M4.5：Selector 调优/差分**

- bitset 调优（u64 word 操作、popcount、前缀复用）；
- 高度 selective 的 compound 用 id/class/tag 倒排缩小 seed，再进入集合代数；
- 与逐元素 matcher 做差分：同一批 selector，结果集/顺序必须完全一致；
- 测量“单 selector / 多 selector / 大文档 / 批量”下的依赖/分支指标，形成基线。

**M5：Python binding**

- PyO3 类、GIL 释放、异常映射；
- `pyproject.toml` + maturin；
- 与 parsel 的 API 对照测试；
- 一个 Scrapy spider 的 drop-in demo。

**M6：硬化/调优**

- cargo-fuzz + sanitizer；
- 内存/分配优化（SoA、`SmallVec`、arena）；
- 批量 CPU（rayon）+ 小文档快路径；
- 文档、benchmark 脚手架（只做 microbench，不做对比）。

**M9：对比评测**

- Python harness：lxml/parsel/selectolax/bs4；
- 合成 + 真实 + 畸形语料；
- parse/select/冷启动/RSS/正确性/依赖/分支指标；
- 报告 + 复现脚本；
- 根据结果回头调 M1–M6 的参数。

### 12.3 为什么是这个顺序

1. **Stage-1 先于 DOM**：stage-1 定义 `MarkupIndex`，是 tokenizer 的输入；先把它做对，后面才有稳定的契约。
2. **FlatDoc 先于 Selector**：选择器依赖 `parent`/`subtree_end`/`sibling_index`；没有扁平文档，选择器无从谈起。
3. **Selector 的集合代数是主路径**：它在 CPU 上就打破 per-element 回溯；逐元素 matcher 只做 fallback 和差分 baseline。
4. **Python binding 在 selector 之后**：binding 是交付物，也是拿真实 Scrapy 页面做反馈的最短路径。
5. **对比评测最后**：先把东西做出来，再和 lxml/parsel/selectolax 算总账；开发期只做 microbench 和正确性，避免过早优化。

### 12.4 关键路径与并行

```text
v1: M0 → M1 → M2 → M3 → M4 → M4.5 → M5 → M6 → M9
```

- M1/M2 期间可以并行做 CSS parser 原型（M4 的一部分）；
- M5 的 binding 骨架可以在 M3 后就开始（先 parse-only），M4 完成后再接 selector；
- GPU 阶段不在这个图里，见第 13 节。

---

## 13. 后续阶段：GPU（v2，独立立项）

> 这一节是 **v1 交付之后** 的内容，不属于当前排期。v1 不为它做任何抽象（没有 `Backend`、没有 `wjsel-cuda`、没有 dispatch 阈值），只保留 `MarkupIndex`/`FlatDoc`/`MatchRecord` 的字段语义作为将来对齐的契约。是否进入本阶段、什么时候进入，由 M9 的结论决定。

### 13.1 先决定：GPU 不做“CPU 代码的另一种编译”

SIMD 与 CUDA 是两条路线：

- **指令集不通用**：`vpshufb`、`pclmulqdq`、`movemask`/`tzcnt` 在 GPU 上没有等价物；
- **并行分解单位不同**：CPU 是 64B 块内位并行 + 块间常数 carry；GPU 是 4B word 一个线程 + 跨 word 的 segmented scan / stable sort，靠批量文档填满；
- **依赖消解方式不同**：CPU 用位切片/CLMUL；GPU 用 `popc + exclusive_scan + 5×shift/xor` 模拟 prefix-XOR、`__clz` 反向 carry、`stable_sort_by_key` 配对；
- **唯一共享的是语义**：`MarkupIndex`（tag_starts/tag_ends/flags）、`FlatDoc`（字段与 `subtree_end` 定义）、`MatchRecord`；代码、kernel、循环结构都不共享。

所以 GPU 阶段是对同一语义契约的**独立实现**，用 CPU 的 conformance test 约束，而不是“把 CPU 算法翻译成 CUDA”。

### 13.2 cuJSON 能借鉴什么

看 `Refs/cuJSON/cujson/parse_standard_json.cu` 和论文（ASPLOS'26）：

- UTF-8 验证：ASCII 快速跳过；每线程两个 4 字节 word，`__vcmpeq4/__vcmpltu4` 做 2/3/4 字节错误模式匹配（Algorithm 1/2）；
- Tokenization：`__vcmpeq4` 建 structural/quotes/backslash 位图 → `__clz(~word)` 反向计数 → `popc + exclusive_scan + 5×shift/xor` 模拟 prefix-XOR → compaction 出 structural index；
- 结构识别：±1 → inclusive scan 得 depth → `stable_sort_by_key(depth)` → XOR 0x06 校验 → expand 成 pairing index；
- 工程：kernel fusion（tokenization 10→6）、pinned + 多 stream；
- 边界：< 8 MB 时慢于 simdjson；峰值显存 2–3× 输入；H2D 最贵；查询仍在 CPU。

可迁移：bitmap-centric、scan/sort 建嵌套、pairing index 思路、按批并行。
不可直接迁移：HTML 的“开/闭”是标签名匹配，且有空元素/可选结束标签——必须先规范化成 Dyck 序列。

### 13.3 GPU 解析管线（草案）

| kernel | 输入 | 输出 | 说明 |
|---|---|---|---|
| `utf8_validate` | bytes + offsets | error flag | ASCII 快速跳过；cuJSON Algorithm 1/2 |
| `classify` | bytes | bitmap word | 4 字节/线程，`__vcmpeq4`；warp ballot 合并 |
| `raw_mask` | bytes | raw bitmap | 先在 CPU 预处理，GPU 只读 |
| `tag_state` | bitmaps | in_tag/in_quote | 2–3 轮 prefix-XOR scan |
| `extract_tags` | bitmaps | tag_starts/ends | stream compaction |
| `tokenize_tags` | bytes + tag spans | TagToken/Attr | 一个线程一个标签 |
| `normalize_flatdoc` | TagToken/Attr | FlatDoc 数组 | 一个线程一篇文档；栈建 parent/children/subtree_end |
| `pairing` | 规范化事件流 | depth/pair_end | 可选：单篇超大文档用 scan+sort |
| `bitset_*` | FlatDoc + selector plan | bitset | 与 8.6 同一套集合代数 |
| `select_matches` | 结果 bitset + FlatDoc | `MatchRecord[]` | compaction；**D2H 只回传这个** |

`tokenize_tags` 一标签一线程：标签边界确定后，标签内部没有跨标签依赖。
`pairing` 取舍：HTML 的规范化本身是栈，batch 路径一文档一线程顺手建 FlatDoc，pairing 免费；scan+sort 只在单篇超大文档时作为选项。

### 13.4 GPU 为什么必须是批量

cuJSON 实测：文件 < 8 MB 时 GPU 比 simdjson 慢（H2D + launch）。网页通常 10 KB–1 MB，单篇上 GPU 必亏。

- 一次传几千篇文档（拼接 buffer + `doc_offsets`）；
- 每个 kernel 处理整个 batch，H2D 只传一次；
- 每篇文档的状态用 segmented scan 在文档边界重置。

API 上不做透明切换：`Selector(text=...)` 永远 CPU；GPU 只走显式的 `select_many(docs, query)` / `parse_many(docs)`。单篇超大文档要显式 `backend="cuda"`。

### 13.5 GPU 查询：set-at-a-time（与 8.6 同一套计划）

- **block-per-doc**：一个 block 跑完一篇文档的 selector 链；bitset 小（10 万节点约 12.5 KB），放 shared memory；
- kernel 序列：`bitset_compound` → `bitset_and/or/not` → `bitset_ancestors`（前缀 + 区间）→ `bitset_parents`/`bitset_prev_sibling`（scatter）→ `bitset_earlier_siblings`（segmented suffix OR）→ `select_matches`；
- 单篇超大文档：一个 block 装不下，拆 kernel + grid sync，或回退 CPU；
- **不返回 DOM**：D2H 只有 `MatchRecord[]`；host bytes 用于 `get()` 物化。

### 13.6 parsel 兼容（GPU 版）

- `GpuDoc` 句柄持有 device 上的 `FlatDoc` 数组 + host 上的原始 bytes；
- `SelectorList` 持有 `Arc<GpuDoc>` + `Vec<MatchRecord>`；
- `.css()` 链式：把当前结果的子树并集作为 root mask，与新的 `M(S)` 相交，再跑一遍集合代数；
- `.get()`/`::attr`/`::text` 在 host 用 span 物化；`.xpath()` 回退 CPU（必要时按需缓存 host `FlatDoc`）；
- 不支持的 selector 子集回退 CPU 逐元素 matcher。

### 13.7 工程细节（草案）

- pinned host memory + 多 stream：H2D、kernel、D2H 重叠；
- 分块（如 64 MB）+ 双缓冲；
- kernel fusion：`classify + tag_state`、`tokenize + attr`；
- `cudarc` 或 `build.rs` + nvcc + `extern "C"`（推荐后者）；
- GPU 端跑 tokenizer/normalizer/flat doc builder；正确性通过 conformance test 对齐 CPU；
- dispatch 阈值是起点：`batch >= 64 MiB` 走 GPU batch，`single_doc >= 8 MiB` 再评估，其余 CPU。

### 13.8 GPU 阶段验收线（进入后适用）

| 项 | 目标 |
|---|---|
| 解析一致性 | GPU `MarkupIndex`/`FlatDoc` 与 CPU 逐字段一致 |
| 查询一致性 | GPU `MatchRecord` 与 CPU 集合代数结果集/顺序一致 |
| 批量端到端 | 聚合 ≥64 MiB 时 `parse+select` ≥ 2× CPU |
| 单文档 | < 8 MiB 不启用；≥ 8 MiB 再评估 |
| 资源 | 显存约 2–3× 输入（参考 cuJSON），峰值 RSS 不高于 CPU 的 1.5× |

### 13.9 GPU 风险

| 风险 | 缓解 |
|---|---|
| 开发机无 GPU，无法日常验证 | CPU conformance baseline + 远程 GPU CI |
| H2D 成为瓶颈 | 批量 + pinned + 多流；实测 go/no-go |
| GPU 查询子集不全 | 先做 ` ` / `>` / `+` / `~` / `:not` / `:is`；其余回退 |
| normalizer 一文档一线程，单篇超大文档慢 | 批量无碍；单篇回退 CPU 或后续并行栈 |
| bitset 显存 | block-per-doc、分块、bitset 复用 |

---

## 14. 风险清单（v1）

| 风险 | 影响 | 缓解 |
|---|---|---|
| HTML 树构造语义复杂 | 结果与 lxml 不一致 | 窄口径 + 规则表 + flag + html5ever 回退 |
| 可选结束标签规则不全 | `subtree_end` 错、选择结果错 | 只做常见 10 条；差分测试；fallback |
| Rust SIMD 可移植性 | 不同 CPU 性能差异 | `std::arch` + runtime dispatch；scalar fallback；NEON 单独路径 |
| parsel XPath 兼容 | 用户迁移受阻 | 先做子集；文档标注；后续用 `sxd-xpath` 或自研扩展 |
| 非 UTF-8 页面 | 解析失败/乱码 | `encoding_rs` 预解码；BOM/meta 检测 |
| 内存膨胀 | 大页面 OOM | 位图不落盘；span 代替字符串；节点/属性上限 |
| 集合代数子集不全 | 某些 selector 走逐元素 | 逐元素 fallback；覆盖 ` ` / `>` / `+` / `~` / `:not` / `:is` |
| 集合代数在 CPU 上不划算 | 小文档/高 selective 查询反而慢 | M4.5 用 seed 索引缩小候选；与逐元素 matcher 做差分和性能对照 |
| 过早优化 | 进度失控 | M9 才做对比；每阶段只设 microbench 门槛 |

---

## 15. 需要拍板的几个问题

1. **默认模式**：`Safe`（flag 时回退 html5ever，兼容性好）还是 `Fast`（best-effort，性能好）？我倾向 `Safe` 做默认，`Fast` 给爬虫显式开启。
2. **XPath 范围**：v1 只做子集（推荐），还是必须覆盖 parsel 的绝大部分 XPath？后者会显著增加 M4 工期。
3. **非 UTF-8**：v1 用 `encoding_rs` 预解码（推荐），还是先只支持 UTF-8？
4. **`:has()`**：v1 是否必须？集合代数其实很适合 `:has`，可以放进 M4 的 CSS 子集。
5. **GPU 是否立项**：等 M9 用“依赖/分支 + 冷启动 + 批量”指标证明 CPU 已经到位后，再决定是否进入第 13 节。

---

## 附录 A：选择器子集清单

**CSS**

- 类型/`*`/`.class`/`#id`
- 属性：`[a]`、`[a=v]`、`^=`、`$=`、`*=`、`~=`、`|=`、`i`
- 组合器：` `、`>`、`+`、`~`
- 伪类：`:first-child`、`:last-child`、`:only-child`、`:nth-child()`、`:nth-last-child()`、`:first-of-type`、`:last-of-type`、`:nth-of-type()`、`:not()`、`:is()`、`:where()`、`:root`、`:empty`
- 伪元素：`::text`、`::attr()`

**XPath（子集）**

- `/`、`//`、`*`、`name`、`@attr`、`text()`
- `[n]`、`[@a]`、`[@a='v']`、`[contains(@a,'v')]`、`[starts-with(@a,'v')]`
- `and` / `or`
- `normalize-space()`、`string()`（可选）

---

## 附录 B：参考代码位置

| 内容 | 路径 |
|---|---|
| simdjson 字符串/转义 | `Refs/simdjson/src/generic/stage1/json_string_scanner.h` |
| simdjson 分类 | `Refs/simdjson/src/haswell.cpp` |
| simdjson prefix_xor | `Refs/simdjson/include/simdjson/haswell/bitmask.h` |
| simdjson UTF-8 | `Refs/simdjson/src/generic/stage1/utf8_lookup4_algorithm.h` |
| html5ever tokenizer 状态 | `Refs/html5ever/html5ever/src/tokenizer/states.rs` |
| html5ever insertion mode | `Refs/html5ever/html5ever/src/tree_builder/types.rs` |
| html5lib 测试数据 | `Refs/html5ever/rcdom/custom-html5lib-*`、`wpt-tree-construction-tests` |
| Stylo 匹配主循环 | `Refs/stylo/selectors/matching.rs`（`matches_complex_selector_internal`） |
| Stylo ancestor bloom | `Refs/stylo/selectors/bloom.rs` |
| Stylo nth-index cache | `Refs/stylo/selectors/nth_index_cache.rs` |
| Stylo selector AST | `Refs/stylo/selectors/parser.rs` |
| parsel API | `Refs/parsel/parsel/selector.py` |
| parsel CSS 翻译 | `Refs/parsel/parsel/csstranslator.py` |
| —— 以下仅 GPU 阶段（第 13 节）参考 —— | |
| cuJSON 三阶段 | `Refs/cuJSON/cujson/parse_standard_json.cu` |
| cuJSON 论文 | `Refs/cuJSON.pdf` / `Refs/cuJSON.zh.md` |

---

## 附：一句话总结

**v1 只有 CPU：simdjson 的 stage-1 把 HTML 变成标签索引，规范化后用 stack 建先序 `FlatDoc`，selector 用 set-at-a-time 集合代数打破 per-element 回溯，逐元素 matcher 只做 fallback；GPU（cuJSON 风格的批量解析 + 同一套集合代数）是 v1 之后独立立项的阶段，v1 不为它预留任何抽象。**

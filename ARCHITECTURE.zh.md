# simdhtml / simdsel 当前架构与状态

`simdhtml` 是面向抓取场景的只读 HTML parser，`simdsel` 是它的 CSS selector engine；`simdsel-py` 提供 parsel 形状的 Python API。目标是只读、零拷贝、不构建浏览器 DOM。

本文只描述**当前实现**，不描述理想化设计。旧文档中已经过时的 RTL、TagId、全局 bitmap cache、cost-based planner 等内容已移除。

## 1. 目录与角色

```text
simdsel/
├── crates/
│   ├── simdhtml/        stage-1 扫描、tokenize、normalize、FlatDoc、SIMD 内核
│   ├── simdsel/         CSS AST、compound mask、combinator、projection、Selector
│   └── simdsel-py/      PyO3 binding
│       ├── python/      Python package + tests
│       └── benchmarks/  Python benchmark harnesses
├── target/bench/        生成 benchmark 结果（gitignore）
├── scripts/             check / bench 脚本
└── fuzz/                fuzz targets
```

## 2. 当前流水线

```text
bytes
  -> UTF-8 validate
  -> stage-1：64B block 分类，记录 tag/text/raw span
  -> tokenize：切 tag name、attribute key/value span、per-token tag code
  -> normalize + Builder：单 pass，open stack + implied end + table wrapper
       -> preorder FlatDoc
  -> selector：
       compound masks
       -> LTR window propagation
       -> final BitSet
       -> MatchRecord / text / attr projection
```

### 2.1 stage-1

`simdhtml::simd` 做：

- 64 字节 block 读取；
- `< > / " ' ! ? alpha` 等字节分类；
- `prefix_xor`：PCLMULQDQ 或 shift/xor fallback；
- 生成 tag span、text run、rawtext/comment 区间。

stage-1 只记录位置，不保留 per-block bitmap。

### 2.2 tokenize

当前 `Tokenized`/`FlatDoc` 中同时存在：

- `tags: Vec<TagToken>`（tag token 本身仍是 AoS）；
- `tag_name_start: Vec<u32>` / `tag_name_len: Vec<u16>`；
- `tag_code: Vec<TagCode>`（normalize 规则使用的 per-token 小整数）；
- `attr_tag` / `attr_key_start` / `attr_key_len` / `attr_value_start` / `attr_value_len` / `attr_quote`。

关键点：

- 没有全局 `TagId`；
- 没有 `tag_names` 字符串 intern 表；
- `tag_code` 只是每个 token 的局部分类，不是全局 tag 表；未知 tag 仍是 `Other` + name span 比较；
- attribute 不解析成语义 key/value，只切结构 span；
- `TagToken` 只保存 span、kind、attrs range、node、self_closing。

### 2.3 normalize + Builder

`normalize` 仍是 tokenize 之后的结构整理阶段，但**不再先物化 Event 流**：

```text
tag token stream + lazy text runs
  -> open stack（TagRef）
  -> implied end（TagCode 整数 switch）
  -> minimal table wrapper
  -> 直接驱动 Builder（EventSink）
  -> preorder FlatDoc
```

Builder 只接收已经配平的流，因此：

- end event 直接关闭当前栈顶，不再做 name 查找 / `rposition`；
- synthetic wrapper 也走同一套栈；
- `synthetic_nodes` 线性查找已删除。
它支持：

- `li`、`p`、`tr`、`td`、`th`、`thead`、`tbody`、`tfoot`、`colgroup` 等常见 implied end；
- `tr` 直接在 `table` 下时插入 `tbody`；
- `td`/`th` 直接在 `table` 下时插入 `tbody`/`tr`；
- `col` 直接在 `table` 下时插入 `colgroup`。

它仍不是完整浏览器 tree construction，也不是 tokenizer 驱动的在线 tree builder：

- 没有 foster parenting；
- 没有完整 insertion modes；
- 没有 `template` content；
- 没有 adoption agency；
- 只覆盖常见 table 结构。

当前 table 规则已改成 `TagCode` switch；普通 start tag 不会进入 table wrapper 的栈扫描。剩余成本主要来自 open-stack 规则本身、节点写入和 stage-1。

### 2.4 Builder / FlatDoc

`FlatDoc` 是 preorder 扁平数组：

```text
FlatDoc {
    input: Box<[u8]>,        // 原始 bytes，永远不改
    nodes: Vec<Node>,        // preorder
    tags: Vec<TagToken>,
    tag_name_start / tag_name_len,
    base_tag_count / synthetic_names,
    attr_* SoA arrays,
    root, flags, warnings
}
```

`Node` 只保留结构：

```text
kind, parent, first_child, next_sibling, prev_element_sibling,
subtree_end, depth, span, tag_token, outer_end, raw
```

没有 `sibling_index`、没有 `type_index`。这些 CSS 查询索引已经移到查询侧。

关键不变量：

```text
节点 i 的后代 = [i + 1, subtree_end[i])
```

### 2.5 synthetic node

table wrapper 是虚拟节点，不是字符串改写：

```text
<table><tr>
  -> normalize emit StartSynthetic("tbody", position..position)
  -> Builder 创建一个零长度 span 的 Node
  -> FlatDoc.synthetic_names 保存 "tbody"
```

input 不变，`get()`/`outer_html()` 仍回读原始 HTML span。  
所以 `table > tbody > tr` 结构匹配正确，但 synthetic tbody 不会在序列化中凭空出现。

## 3. selector 当前执行模型

### 3.1 AST

CSS 编译成：

```text
Selector
  Complex
    Compound[]
    Combinator[]
```

Compound 静态部分：

```text
tag / id / class / attrs / pseudos
```

### 3.2 compound mask

第一个 compound 可能全局生成 mask：

- tag selector：AVX2 tag mask（见优化技巧）；
- id/class/attr：候选节点上的 span 查询；
- pseudo：在候选 mask 上过滤。

### 3.3 LTR window propagation

主路径是 preorder + LTR，不使用 cost-based planner：

```text
current = compound_points(C0)
for combinator:
    windows = relation_windows(current, combinator)
    current = compound_points_in_windows(Ci+1, windows)
```

窗口来源：

| CSS | 窗口 |
|---|---|
| `A B` | `[id+1, subtree_end[id])` |
| `A > B` | `first_child` / `next_sibling` |
| `A + B` | next element sibling |
| `A ~ B` | 后续 sibling 窗口 |
| `preceding-sibling` | `prev_element_sibling` |

中间结果是 `Vec<NodeId>` 点集，不构造全局 `BitSet(N)`；最终一个 complex 结束时才转一次 BitSet。

### 3.4 查询侧 child/type 索引

`:nth-child` 家族使用查询局部 `ChildIndex`：

```text
child_start, children, element_child_count, element_index
```

`:nth-of-type` 家族：

- 显式 tag，例如 `p:nth-of-type(2)`：
  - 使用 `tag_mask("p")`；
  - 在 parent child group 内做 prefix/popcount；
- `*:nth-of-type(2)`：
  - 走 per-node tag name fallback；
  - 这是特殊路径，不影响普通 `p:nth-of-type`。

### 3.5 chain cache

`QueryCache` 是分层链：

```text
QueryCache {
    parent: Option<Rc<QueryCache>>,
    tag_masks,
    bare_compounds,
    child_index,
}
```

规则：

- 链式 `.css()` 创建 child layer，可以读 parent；
- 平级 `.css()` 各自创建 child，新增项互不可见；
- 没有全局 bitmap cache；
- 查询结束，整条 Rc 链释放。

## 4. 设计原则

### 4.1 结构先切，语义后做

例：

```html
<div class="item" data-id="1">
```

第一轮只保留：

```text
tag name span "div"
class key/value span
data-id key/value span
```

不在 parse 时：

- intern `div` / `class` / `data-id`；
- 建全局 attribute 表；
- 计算所有 class token。

查询时：

```css
div.item[data-id="1"]
```

才在 span/SoA 上做 tag/attr/class 匹配。

### 4.2 不做全局 tag/attribute intern

没有：

```rust
TagId
tag_names: Vec<Box<str>>
attr_names
```

tag 身份是：

```text
tag_token index
+ tag_name_start/len
+ synthetic_names（仅 table wrapper）
```

未知 tag 才做 name span 比较；已知 selection 通过 tag mask。

### 4.3 preorder 窗口优先

例：

```css
.item .title
```

不是：

```text
global descendants(.item) -> BitSet(N)
global .title -> BitSet(N)
AND
```

而是：

```text
.item -> 节点集合
-> 每个 item 的 [id+1, subtree_end[id]) 窗口
-> 只在窗口里找 .title
```

### 4.4 CSS 索引查询时算

例：

```css
li:nth-child(2n+1)
p:nth-of-type(2)
```

不在 parser 里预计算 `sibling_index` / `type_index`。  
`:nth-child` 用查询侧 `element_index`；`:nth-of-type` 用 tag mask + child group prefix。

### 4.5 不做过早优化

- 不把 short attribute name 的第一轮结构切分做成 SIMD；
- 不为了 `class` 的 value 做 SIMD 字符串比较；
- SIMD 只用在批量 tag/attribute key mask、bitmap 类操作上；
- 大窗口是用户 selector 的问题，不做自动窗口合并。

### 4.6 单线程、共享用 Rc

没有 `Arc`、没有 `rayon`。  
`Selector` 持有 `Rc<FlatDoc>` / `Rc<QueryCache>`；Python binding 是 `unsendable`，`parse_many` 顺序执行。

## 5. 当前优化技巧

### 5.1 stage-1 SIMD

- 64B block；
- `vpshufb`/`cmpeq` 分类；
- `prefix_xor` 用 CLMUL；
- 只产出位置，不保留 bitmap；
- raw scan 的 tag-name 判定按长度过滤，只对候选名做 case-insensitive 比较；
- raw scan 的 `find_tag_end` 只在引号外逐字节扫描，引号值用 `memchr` 跳过；`-->` / `]]>` 复用缓存的 `memmem::Finder`；
- text runs 与 raw/non-text ranges 都是有序的，因此用线性 union，不再对每个 tag 排序。

### 5.1.1 normalize + Builder 单 pass

normalize 通过 `EventSink` 直接驱动 Builder：

- 不分配 `Vec<Event>`；
- end 直接关栈顶，不做 name 查找；
- 只保留一条 normalized open stack 和一条 node open stack，顺序天然一致。

### 5.2 通用 key mask（tag name / attribute key）

tag name 和 attribute key 共用同一个 `key_mask` kernel，输入是 SoA 的
`start/len` 与“key index -> owner node”的解析函数。

name_len <= 4：

```text
load 8×u16 name_len -> cvtepu16_epi32 -> cmpeq(len)
i32gather_epi32(name_start) -> fold 0x20 -> byte_mask -> cmpeq
movemask(bytes) & movemask(len)
```

name_len 5..8：

```text
i32gather_epi64
-> fold
-> 64-bit byte_mask
-> cmpeq_epi64
+ SIMD length compare
```

> 8 字节走 scalar；输入尾部 scalar；tag 数 < 64 走 scalar。

### 5.3 attribute key mask

`.class` 之外的属性选择先走 query-side attribute key mask：

- `[data-id]`、`#id`、`[href^=...]` 先在 attribute SoA 上做批量 key 扫描；
- 只在真正带该 key 的元素上做 value 匹配；
- `class` 仍走普通元素扫描，因为普通页面里 class 太普遍，key mask 反而多一遍扫描。

### 5.4 windowed combinator

- `relation_windows` 直接使用 preorder 结构；
- 下一 compound 先和 tag mask / attribute key mask 求交，再逐窗口过滤；
- 中间层 `Vec<NodeId>`；
- 不做全局 `descendants/children/~` bitmap；
- 不做 cost-based / popcount 预检查。

### 5.5 查询侧索引

- `ChildIndex` 提供 child group / element_index；
- `:nth-child` 不再依赖 parser `sibling_index`；
- `:nth-of-type` 显式 tag 用 tag mask prefix；
- `*:nth-of-type` 才 fallback name compare。

### 5.6 chain cache

- 链内继承；
- 平级隔离；
- 避免 `.css(".item") -> .css(".item .title")` 重复算 mask。

## 6. 当前性能状态

最近一轮完整测试：30 KiB / 100 KiB / 400 KiB / 4 MiB，cold parse+select，best of 3。
结果写入 `target/bench/`（gitignore）；历史数据在 `crates/simdsel-py/benchmarks/archive/`。

### 30 KiB

| corpus | parse-only | 代表查询 |
|---|---:|---:|
| cards | 0.097ms | `.item .title::text` 0.220ms |
| table | 0.167ms | `tr:nth-child(2n+1)` 0.187ms |
| list | 0.136ms | `li a::attr(href)` 0.259ms |

### 100 KiB

| corpus | parse-only | 代表查询 |
|---|---:|---:|
| cards | 0.354ms | `.item .title::text` 0.679ms |
| table | 0.657ms | `tr:nth-child(2n+1)` 0.740ms |
| list | 0.622ms | `li a::attr(href)` 0.912ms |

### 400 KiB

| corpus | parse-only | 代表查询 |
|---|---:|---:|
| cards | 1.848ms | `.item .title::text` 2.085ms |
| table | 2.210ms | `tr:nth-child(2n+1)` 3.056ms |
| list | 2.175ms | `li a::attr(href)` 2.913ms |

### 4 MiB

| corpus | parse-only | 代表查询 |
|---|---:|---:|
| cards | 17.617ms | `.item .title::text` 22.041ms |
| table | 16.753ms | `tr:nth-child(2n+1)` 26.185ms |
| list | 15.849ms | `li a::attr(href)` 33.276ms |

benchmark harness：

- `crates/simdhtml/benches/`：microbench + html5ever parse-only 对比（`bench-external`）
- `crates/simdsel/benches/`：cold parse+select + scraper/lol_html 对比（`bench-external`）
- `crates/simdsel-py/benchmarks/`：Python synthetic / Common Crawl / 回归报告
- `target/bench/`：生成结果（gitignore）

结论：

- 当前 parse 在 30 KiB–4 MiB 范围内约为 220–320 MB/s；
- table 的局部规则已经变成 TagCode switch，普通 tag 不再进入 table wrapper 栈扫描；
- cards/list 也受益于 raw scan、text-run 线性合并和单 pass builder；
- selector 侧主要成本已经不是全局 combinator，而是 class-only 首 compound 扫描、attr value 查询、窗口重复。

## 7. 未完成 / 下一阶段

### 7.1 真正的在线 tree builder

当前已经把 normalize 和 Builder 合成单 pass（`EventSink`），但输入仍是：

```text
tokenize 全量
-> normalize + Builder 单 pass 流式驱动
```

还没有做到 html5ever 风格：

```text
tokenizer 逐 token
-> TreeBuilder insertion mode
-> 立即插 synthetic node
-> FlatDoc
```

下一步可以：

- 用 insertion mode 状态替换每次 `find_last(stack, ...)`；
- 把 tokenize 也流式化，只保留当前 tag 的 attribute span；
- 为 foster parenting / template 留出正确位置。

### 7.2 完整 table 兼容

当前只做：

- `tr` -> `tbody`
- `td/th` -> `tbody/tr`
- `col` -> `colgroup`

未做：

- foster parenting；
- `caption` / `colgroup` / `thead` / `tfoot` 的完整 insertion modes；
- nested table；
- template；
- adoption agency。

### 7.3 其他 selector 项

- `*:nth-of-type` 仍是 name fallback；
- `:has` 仍走全局关系，没有窗口化；
- class-only 首 compound 仍逐元素查 `class` 属性；
- 首 compound 仍是全局扫描；
- 窗口不合并，重复区间会重复扫。

### 7.4 数据结构

- `FlatDoc.tags` 仍是 `Vec<TagToken>` AoS；
- `tag_name_start/len`、`attr_*` 已是 SoA，但 tag token / node 本身未完全拆平；
- 没有 full SoA node columns；
- 没有 query-local scratch arena。

## 8. 非目标

- 浏览器级 DOM；
- CSS 层叠 / 计算样式；
- JavaScript；
- GPU 阶段；
- 完整 XML 命名空间；
- 完整 HTML5 tree construction（当前是 best-effort）；
- XPath 接口（见 §10）。

## 9. 验证

```bash
scripts/check.sh

uv run --group dev maturin develop --release
.venv/bin/python crates/simdsel-py/benchmarks/synthetic_corpora.py \
  --sizes 30KiB,100KiB,500KiB \
  --backends simdsel,parsel,selectolax \
  --reps 3 --timeout-seconds 10
```

## 10. XPath 移除决策

simdsel 不提供 XPath 接口，Rust `Selector` / Python `Selector` 均只有 CSS 查询。移除的内容包括：

- `simdsel::Selector::xpath` / `xpath_as` / `xpath_count`；
- `simdsel::parse_xpath`；
- Python `Selector.xpath` / `SelectorList.xpath`；
- XPath parser、XPath 专用 `PrecedingSibling` combinator、`AttrWildcard` 投影。

理由：

- 即使抓取场景，CSS 使用率也远高于 XPath；
- 当前 XPath 只是手写子集，无法宣称 parsel/lxml 兼容；
- 继续维护 XPath parser、axes、predicate、函数和类型转换的成本，远高于 CSS；
- 不提供一个半兼容的 XPath，比提供一个容易误导的 XPath 子集更清晰；
- 需要完整 XPath 1.0 的用户应使用 parsel/lxml。

为了保持 parsel 形状，CSS 侧保留并明确以下非标准扩展：

| 扩展 | 语义 |
|---|---|
| `::text` | 投影匹配元素的直接 text 子节点，和 parsel 的常见用法一致 |
| `::attr(name)` | 投影第一个匹配 attribute 的原始值；`name` 必填 |
| `:contains(text)` | 对元素拼接后的后代文本做子串匹配 |
| `[attr!=value]` | 非标准的 attribute 不等选择器 |

明确不支持：

- `::attr(*)`；
- XPath `@*`；
- XPath axes / predicate / 函数。

动态伪类（`:hover`、`:focus`、`:visited` 等）仍解析为 `Never`，即静态文档中不匹配；这与“直接报错”不同，文档和测试以此为准。

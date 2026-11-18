"""simdsel: parsel-shaped HTML selection backed by a Rust/SIMD core."""

from ._simdsel import Selector, SelectorList, __version__

__all__ = ["Selector", "SelectorList", "__version__"]

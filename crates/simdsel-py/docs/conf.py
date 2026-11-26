from simdsel import __version__

project = "simdsel"
author = "Xubai Wang"
copyright = "2026, Xubai Wang"
release = __version__

extensions = [
    "sphinx.ext.autodoc",
    "sphinx.ext.napoleon",
    "myst_parser",
]

autodoc_member_order = "bysource"
autodoc_typehints = "none"

html_theme = "furo"
html_title = "simdsel"
html_baseurl = "https://xbwwj.github.io/simdhtml/"
html_theme_options = {
    "source_repository": "https://github.com/xbwwj/simdhtml/",
    "source_branch": "main",
    "source_directory": "crates/simdsel-py/docs/",
}

root_doc = "index"
source_suffix = {".rst": "restructuredtext", ".md": "markdown"}
exclude_patterns = ["_build"]

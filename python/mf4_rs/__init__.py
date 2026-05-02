"""Python bindings for the ``mf4-rs`` Rust crate.

The actual symbols live in the compiled extension module ``mf4_rs._mf4_rs``;
this ``__init__.py`` re-exports them so users can ``import mf4_rs`` and reach
``PyMDF``, ``PyMdfWriter``, ``PyMdfIndex`` and the helper functions
unchanged. See ``__init__.pyi`` for type stubs (used by IDEs for hover docs
and autocomplete).
"""

from ._mf4_rs import *  # noqa: F401,F403
from ._mf4_rs import __doc__, __all__  # noqa: F401

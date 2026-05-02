# Re-export every public symbol from the compiled extension module so that
# `import mf4_rs` resolves through to the type information generated in
# `_mf4_rs.pyi`. Keeps IDE hover, autocomplete, and `mypy --strict` happy
# without duplicating signatures here.
from ._mf4_rs import *  # noqa: F401,F403

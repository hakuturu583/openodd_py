"""ODDs written in Python, registered by name, and how an application picks one.

An ODD written in Python is a function that takes nothing and returns an
:class:`~openodd.OddDefinition` -- its attributes carry their own probes, so
it needs no binding file. An application lets its user pick an ODD by a
string (a config key, a command-line argument), and :func:`resolve_odd` reads
the string as one of:

* ``"default"`` (or nothing): the bundled ``default`` ODD
  (:func:`~openodd.bundled_odd`), and likewise the name of any other ODD this
  package ships (:data:`~openodd.BUNDLED_ODDS`);
* a name registered with :func:`register_odd`, in code or by a package through
  the ``openodd.odds`` entry point group (the entry point names a function
  that takes nothing and calls :func:`register_odd`);
* a path to a ``.yaml`` / ``.yml`` file: a binding file, or an OpenODD
  document on its own (:func:`~openodd.load_odd_file`; nothing is measured);
* ``"package.module:function"``: a builder to import and call.

The files an ODD is read from -- a bundled ODD, a ``.yaml`` file -- are
measured by the *backend* :func:`resolve_odd` is given (else the default
one); a Python ODD brings its probes with it. An application that keeps an
entry point group of its own adds it with :func:`add_entry_point_group`.
"""

from __future__ import annotations

import logging
import os
from collections.abc import Callable
from typing import Optional, Union

from .backend import Backend, import_callable
from .loader import BUNDLED_ODDS, bundled_odd, load_odd_file
from .model import OddDefinition

__all__ = [
    "DEFAULT_ODD",
    "ENTRY_POINT_GROUP",
    "OddBuilder",
    "OddSpec",
    "add_entry_point_group",
    "odd_builder",
    "odd_names",
    "register_odd",
    "registered_odds",
    "resolve_odd",
    "unregister_odd",
]

logger = logging.getLogger(__name__)

#: The name of the ODD picked when none is named.
DEFAULT_ODD = "default"

#: Entry point group a package registers ODD builders under.
ENTRY_POINT_GROUP = "openodd.odds"

#: Takes nothing, returns the ODD.
OddBuilder = Callable[[], OddDefinition]
#: What :func:`resolve_odd` takes.
OddSpec = Union[str, "os.PathLike[str]", OddDefinition, None]

_REGISTRY: dict[str, OddBuilder] = {}
_GROUPS: list[str] = [ENTRY_POINT_GROUP]
_LOADED_GROUPS: set[str] = set()
#: The entry point being loaded, if any: what it registers never replaces a name.
_LOADING: list[str] = []


def register_odd(name: str, builder: OddBuilder) -> None:
    """Register *builder* (taking nothing, returning an ODD) under *name*.

    A name registered in code replaces one an entry point registered; an
    entry point never replaces a name already registered, whichever loaded
    first.

    Raises:
        ValueError: if *name* is empty, has surrounding whitespace, ends in
            ``.yaml`` / ``.yml`` (it would be read as a file), or names a
            bundled ODD.
    """
    if not name:
        raise ValueError("register_odd(): name must not be empty")
    if name != name.strip():
        raise ValueError(f"register_odd(): {name!r} has surrounding whitespace")
    if _is_yaml(name):
        raise ValueError(f"register_odd(): {name!r} would be read as a file")
    if name in BUNDLED_ODDS:
        raise ValueError(f"register_odd(): {name!r} is a bundled ODD")
    previous = _REGISTRY.get(name)
    if previous is not None and previous is not builder:
        if _LOADING:
            logger.warning(
                "ODD plugin %r registers %r, which is registered already; keeping that one",
                _LOADING[-1],
                name,
            )
            return
        logger.warning("register_odd(): replacing the ODD registered as %r", name)
    _REGISTRY[name] = builder


def unregister_odd(name: str) -> Optional[OddBuilder]:
    """Forget the ODD registered under *name*; return its builder, if any."""
    _load_plugins()  # else an entry point would register it again later
    return _REGISTRY.pop(name, None)


def registered_odds() -> dict[str, OddBuilder]:
    """The registered ODDs (entry points loaded), name -> builder."""
    _load_plugins()
    return dict(_REGISTRY)


def add_entry_point_group(group: str) -> None:
    """Also load the ODDs registered under the entry point *group*.

    For an application that has advertised a group of its own; its entry
    points name, as ``openodd.odds`` ones do, a function that takes nothing
    and calls :func:`register_odd`.
    """
    if not group:
        raise ValueError("add_entry_point_group(): group must not be empty")
    if group not in _GROUPS:
        _GROUPS.append(group)


def _load_plugins() -> None:
    import importlib.metadata as importlib_metadata

    for group in _GROUPS:
        if group in _LOADED_GROUPS:
            continue
        _LOADED_GROUPS.add(group)
        for entry_point in importlib_metadata.entry_points(group=group):
            before = dict(_REGISTRY)
            _LOADING.append(entry_point.name)
            try:
                entry_point.load()()
                logger.info("Loaded ODD plugin %r from %s", entry_point.name, entry_point.value)
            except Exception:  # one bad plugin must not stop the rest
                # Nothing of a plugin that failed half way is kept.
                _REGISTRY.clear()
                _REGISTRY.update(before)
                logger.exception("Failed to load ODD plugin %r; skipping.", entry_point.name)
            finally:
                _LOADING.pop()


def odd_names() -> list[str]:
    """The names an ODD can be picked by: the bundled ones, then the registered ones."""
    _load_plugins()
    bundled = [DEFAULT_ODD, *sorted(n for n in BUNDLED_ODDS if n != DEFAULT_ODD)]
    return [*bundled, *sorted(_REGISTRY)]


def _is_yaml(text: str) -> bool:
    return text.lower().endswith((".yaml", ".yml"))


def odd_builder(spec: OddSpec) -> Optional[OddBuilder]:
    """The Python builder *spec* names, or ``None`` for a bundled ODD or a file.

    Raises:
        ValueError: if *spec* names nothing, or is a path that is not YAML.
    """
    if spec is None or isinstance(spec, OddDefinition):
        return None
    if isinstance(spec, os.PathLike):
        path = os.fspath(spec)
        if not _is_yaml(str(path)):
            raise ValueError(f"{path}: an ODD file is .yaml or .yml")
        return None
    text = str(spec).strip()
    if not text or text in BUNDLED_ODDS or _is_yaml(text):
        return None
    _load_plugins()
    if text in _REGISTRY:
        return _REGISTRY[text]
    if ":" in text:
        return import_callable(text)
    raise ValueError(
        f"no ODD named {text!r}; known: {', '.join(odd_names())}, "
        "a .yaml file, or package.module:function"
    )


def resolve_odd(spec: OddSpec = None, *, backend: Optional[Backend] = None) -> OddDefinition:
    """The ODD *spec* names (see the module docstring).

    Args:
        spec: A name, a ``.yaml`` path, ``package.module:function``, an ODD
            (returned as is), or ``None`` for the default.
        backend: What measures an ODD read from files; the default backend
            when ``None``.

    Raises:
        ValueError: if *spec* names nothing, or its builder does not return an
            :class:`~openodd.OddDefinition`.
    """
    if isinstance(spec, OddDefinition):
        return spec
    builder = odd_builder(spec)
    if builder is not None:
        odd = builder()
        if not isinstance(odd, OddDefinition):
            raise ValueError(f"ODD builder {spec!r} returned {type(odd).__name__}")
        return odd
    text = os.fspath(spec).strip() if spec is not None else ""
    if text and _is_yaml(text):
        return load_odd_file(text, backend=backend)
    return bundled_odd(text or DEFAULT_ODD, backend=backend)

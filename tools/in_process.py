"""A tool's main() run in a test's own process, as `python3 -B TOOL ARGS` runs it
(#525): one interpreter per planted case was most of a test file's seconds. The
tool is loaded once from its file; each run sets the module globals it names
(the tool's ROOT), sys.argv (and main's argv, when main takes one) and a
captured stdout and stderr, and returns the exit code by sys.exit's rules: None
is 0, an int is itself, any other value is printed to stderr and is 1, and an
uncaught error prints its traceback and is 1.
A test that uses it keeps one case that runs the script itself beside it and
asserts the two agree byte for byte.
"""

import contextlib
import importlib.machinery
import importlib.util
import inspect
import io
import sys
import traceback

_LOADED = {}


def load(path):
    """The tool at `path` as a module, loaded once per process."""
    path = str(path)
    if path not in _LOADED:
        name = "in_process_" + str(len(_LOADED))
        loader = importlib.machinery.SourceFileLoader(name, path)
        mod = importlib.util.module_from_spec(importlib.util.spec_from_loader(name, loader))
        loader.exec_module(mod)
        _LOADED[path] = mod
    return _LOADED[path]


def run(path, args, **globals_):
    """(exit code, stdout, stderr) of the tool at `path` run with `args`."""
    mod = load(path)
    for key, value in globals_.items():
        setattr(mod, key, value)
    out, err = io.StringIO(), io.StringIO()
    argv, sys.argv = sys.argv, [str(path), *map(str, args)]
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = mod.main(sys.argv[1:]) if inspect.signature(mod.main).parameters else mod.main()
            except SystemExit as e:
                code = e.code
            except Exception:  # the script's uncaught error: its traceback, exit 1
                traceback.print_exc()
                code = 1
            if code is not None and not isinstance(code, int):
                print(code, file=sys.stderr)
                code = 1
    finally:
        sys.argv = argv
    return code or 0, out.getvalue(), err.getvalue()

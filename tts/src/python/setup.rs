//! Python interpreter setup — venv detection and sys.path configuration.

use pyo3::prelude::*;

/// Add the active venv's site-packages to `sys.path` and reset `sys.argv`.
///
/// Reads `$VIRTUAL_ENV` (set automatically by `source .venv/bin/activate`)
/// and derives the site-packages path from the running Python version.
/// If `$VIRTUAL_ENV` is not set, this is a no-op — system Python is used.
pub fn setup(py: Python) -> PyResult<()> {
    let venv = match std::env::var("VIRTUAL_ENV") {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let version = py.eval(c"__import__('sys').version_info[:2]", None, None)?;
    let (major, minor): (u8, u8) = version.extract()?;

    let site_packages = format!("{venv}/lib/python{major}.{minor}/site-packages");

    let sys = py.import("sys")?;

    let argv = sys.getattr("argv")?;
    argv.call_method0("clear")?;
    argv.call_method1("append", ("",))?;

    sys.getattr("path")?
        .call_method1("insert", (0, &site_packages))?;

    Ok(())
}

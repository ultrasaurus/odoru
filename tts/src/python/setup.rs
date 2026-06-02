//! Python interpreter setup — venv detection and sys.path configuration.

use pyo3::prelude::*;

pub fn setup(py: Python) -> PyResult<()> {
    let venv = match std::env::var("VIRTUAL_ENV") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("  [setup] VIRTUAL_ENV not set — using system Python");
            return Ok(());
        }
    };

    eprintln!("  [setup] VIRTUAL_ENV = {venv}");

    let version = py.eval(c"__import__('sys').version_info[:2]", None, None)?;
    let (major, minor): (u8, u8) = version.extract()?;

    let site_packages = format!("{venv}/lib/python{major}.{minor}/site-packages");
    eprintln!("  [setup] inserting {site_packages}");

    let sys = py.import("sys")?;

    let argv = sys.getattr("argv")?;
    argv.call_method0("clear")?;
    argv.call_method1("append", ("",))?;

    sys.getattr("path")?
        .call_method1("insert", (0, &site_packages))?;

    // Verify it took effect
    let path: Vec<String> = sys.getattr("path")?.extract()?;
    eprintln!("  [setup] sys.path[0] = {:?}", path.first());

    Ok(())
}

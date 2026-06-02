//! # py-venv
//!
//! Shared Python integration utilities for crates that embed Python scripts.
//!
//! ## Usage
//!
//! ```
//! use pyo3::prelude::*;
//! use pyo3::ffi::c_str;
//! use pyo3::types::PyDict;
//!
//! Python::attach(|py| {
//!     py_venv::setup(py).unwrap();
//!
//!     let module = py_venv::embed_module(
//!         py,
//!         c_str!("def greet(name): return f'hello {name}'"),
//!         c"example.py",
//!         c"example",
//!     ).unwrap();
//!
//!     let kwargs = PyDict::new(py);
//!     kwargs.set_item("name", "world").unwrap();
//!
//!     let result = py_venv::call_kwargs(py, &module, "greet", &kwargs).unwrap();
//!     let s: String = result.extract().unwrap();
//!     assert_eq!(s, "hello world");
//! });
//! ```

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};
use std::ffi::CStr;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Acquire the Python GIL and run a closure.
///
/// This is the main entry point for all Python interaction. The closure
/// receives a `Python<'py>` token required by other `py_venv` functions.
///
/// **Never hold across `.await`** — the GIL blocks the OS thread while held.
/// In async code, use `tokio::task::spawn_blocking` to move GIL work off
/// the async executor.
///
/// # Example
/// ```
/// py_venv::with_python(|py| {
///     py_venv::setup(py).unwrap();
///     Ok::<_, pyo3::PyErr>(())
/// }).unwrap();
/// ```
pub fn with_python<F, T, E>(f: F) -> Result<T, E>
where
    F: for<'py> FnOnce(Python<'py>) -> Result<T, E>,
    E: From<PyErr>,
{
    Python::attach(f)
}


/// Add the active venv's site-packages to `sys.path` and reset `sys.argv`.
///
/// Reads `$VIRTUAL_ENV` (set automatically by `source .venv/bin/activate`)
/// and derives the site-packages path from the running Python version.
/// If `$VIRTUAL_ENV` is not set, this is a no-op — system Python is used.
///
/// Also resets `sys.argv` to `[""]` to prevent libraries that parse argv at
/// import time from failing when run under `cargo test`.
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

// ---------------------------------------------------------------------------
// Module embedding
// ---------------------------------------------------------------------------

/// Compile and execute a Python source string embedded in the binary,
/// returning the resulting module object.
///
/// Typically used with `c_str!(include_str!("my_script.py"))` to embed a
/// `.py` file at compile time. The returned `Py<PyAny>` can be stored and
/// reused across GIL acquisitions.
///
/// # Example
/// ```
/// use pyo3::prelude::*;
/// use pyo3::ffi::c_str;
/// Python::attach(|py| {
///     let module = py_venv::embed_module(
///         py,
///         c_str!("def hello(): return 'world'"),
///         c"example.py",
///         c"example",
///     ).unwrap();
/// });
/// ```
pub fn embed_module(
    py: Python,
    code: &CStr,
    filename: &CStr,
    module_name: &CStr,
) -> PyResult<Py<PyAny>> {
    let module = PyModule::from_code(py, code, filename, module_name)?;
    Ok(module.into())
}

// ---------------------------------------------------------------------------
// Calling
// ---------------------------------------------------------------------------

/// Call a function in a module by name, passing all arguments as kwargs.
///
/// Using kwargs-only keeps call sites self-documenting and order-independent.
///
/// # Example
/// ```
/// use pyo3::prelude::*;
/// use pyo3::types::PyDict;
/// use pyo3::ffi::c_str;
/// Python::attach(|py| {
///     let module = py_venv::embed_module(
///         py,
///         c_str!("def add(a, b): return a + b"),
///         c"m.py",
///         c"m",
///     ).unwrap();
///     let kwargs = PyDict::new(py);
///     kwargs.set_item("a", 1).unwrap();
///     kwargs.set_item("b", 2).unwrap();
///     let result = py_venv::call_kwargs(py, &module, "add", &kwargs).unwrap();
///     let sum: i32 = result.extract().unwrap();
///     assert_eq!(sum, 3);
/// });
/// ```
pub fn call_kwargs<'py>(
    py: Python<'py>,
    module: &Py<PyAny>,
    fn_name: &str,
    kwargs: &Bound<'py, PyDict>,
) -> PyResult<Bound<'py, PyAny>> {
    module
        .bind(py)
        .getattr(fn_name)?
        .call((), Some(kwargs))
}

// ---------------------------------------------------------------------------
// Dict extraction helpers
// ---------------------------------------------------------------------------

/// Extract a `Vec<f32>` from a Python dict value by key.
pub fn extract_f32_list(dict: &Bound<PyAny>, key: &str) -> PyResult<Vec<f32>> {
    dict.get_item(key)?.extract()
}

/// Extract a `u32` from a Python dict value by key.
pub fn extract_u32(dict: &Bound<PyAny>, key: &str) -> PyResult<u32> {
    dict.get_item(key)?.extract()
}

/// Extract an `f64` from a Python dict value by key.
pub fn extract_f64(dict: &Bound<PyAny>, key: &str) -> PyResult<f64> {
    dict.get_item(key)?.extract()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::ffi::c_str;
    use pyo3::types::PyDict;

    #[test]
    fn setup_no_virtual_env_is_noop() {
        let saved = std::env::var("VIRTUAL_ENV").ok();
        std::env::remove_var("VIRTUAL_ENV");
        Python::attach(|py| {
            assert!(setup(py).is_ok(), "setup should succeed with no VIRTUAL_ENV");
        });
        if let Some(v) = saved { std::env::set_var("VIRTUAL_ENV", v); }
    }

    #[test]
    fn embed_module_and_call_kwargs() {
        Python::attach(|py| {
            let module = embed_module(
                py,
                c_str!("def greet(name): return f'hello {name}'"),
                c"test.py",
                c"test",
            ).expect("embed_module failed");

            let kwargs = PyDict::new(py);
            kwargs.set_item("name", "world").unwrap();

            let result = call_kwargs(py, &module, "greet", &kwargs)
                .expect("call_kwargs failed");
            let s: String = result.extract().unwrap();
            assert_eq!(s, "hello world");
        });
    }

    #[test]
    fn call_kwargs_with_multiple_args() {
        Python::attach(|py| {
            let module = embed_module(
                py,
                c_str!("def add(a, b): return a + b"),
                c"add.py",
                c"add",
            ).unwrap();

            let kwargs = PyDict::new(py);
            kwargs.set_item("a", 3).unwrap();
            kwargs.set_item("b", 4).unwrap();

            let result = call_kwargs(py, &module, "add", &kwargs).unwrap();
            let sum: i32 = result.extract().unwrap();
            assert_eq!(sum, 7);
        });
    }

    #[test]
    fn extract_f32_list_from_dict() {
        Python::attach(|py| {
            let module = embed_module(
                py,
                c_str!("def get(): return {'samples': [0.1, 0.2, 0.3], 'rate': 24000, 'dur': 1.5}"),
                c"ext.py",
                c"ext",
            ).unwrap();

            let kwargs = PyDict::new(py);
            let result = call_kwargs(py, &module, "get", &kwargs).unwrap();

            let samples = extract_f32_list(&result, "samples").unwrap();
            assert_eq!(samples.len(), 3);
            assert!((samples[0] - 0.1).abs() < 0.001);

            let rate = extract_u32(&result, "rate").unwrap();
            assert_eq!(rate, 24_000);

            let dur = extract_f64(&result, "dur").unwrap();
            assert!((dur - 1.5).abs() < 0.001);
        });
    }

    #[test]
    fn call_kwargs_unknown_function_returns_error() {
        Python::attach(|py| {
            let module = embed_module(
                py,
                c_str!("def foo(): pass"),
                c"m.py",
                c"m",
            ).unwrap();

            let kwargs = PyDict::new(py);
            let result = call_kwargs(py, &module, "nonexistent", &kwargs);
            assert!(result.is_err());
        });
    }
}

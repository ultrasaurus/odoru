//! Pure Python plumbing — module loading and function calling.

use pyo3::prelude::*;
use std::ffi::CStr;

/// Load a Python module from source code embedded as a `&CStr`.
pub fn load_module(
    py: Python,
    code: &CStr,
    filename: &CStr,
    module_name: &CStr,
) -> PyResult<Py<PyAny>> {
    let module = pyo3::types::PyModule::from_code(py, code, filename, module_name)?;
    Ok(module.into())
}

/// Extract a `Vec<f32>` from a Python dict value.
pub fn extract_f32_list(dict: &Bound<PyAny>, key: &str) -> PyResult<Vec<f32>> {
    dict.get_item(key)?.extract()
}

/// Extract a `u32` from a Python dict value.
pub fn extract_u32(dict: &Bound<PyAny>, key: &str) -> PyResult<u32> {
    dict.get_item(key)?.extract()
}

/// Extract an `f64` from a Python dict value.
pub fn extract_f64(dict: &Bound<PyAny>, key: &str) -> PyResult<f64> {
    dict.get_item(key)?.extract()
}

use pyo3::ffi::c_str;
use pyo3::types::PyDict;

fn main() {
    py_venv::with_python(|py| {
        py_venv::setup(py)?;

        let module = py_venv::embed_module(
            py,
            c_str!("def greet(): print('hello world!')"),
            c"hello.py",
            c"hello",
        )?;

        let kwargs = PyDict::new(py);
        py_venv::call_kwargs(py, &module, "greet", &kwargs)?;

        Ok::<_, pyo3::PyErr>(())
    }).expect("Python error");
}

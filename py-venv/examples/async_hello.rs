//! Async example — Python called from a Tokio runtime.
//!
//! The GIL must never be held across `.await`. The correct pattern is:
//!   1. Move Python work into `spawn_blocking` (runs on a thread pool)
//!   2. Await the result
//!   3. Continue with async work
//!
//! This lets Tokio keep scheduling other tasks while Python runs.

use pyo3::ffi::c_str;
use pyo3::types::{PyDict, PyAnyMethods};

#[tokio::main]
async fn main() {
    // Simulate doing some async work before calling Python
    println!("Fetching data...");
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    println!("Data ready, calling Python...");

    // Move GIL work off the async executor into a blocking thread
    let result = tokio::task::spawn_blocking(|| {
        py_venv::with_python(|py| {
            py_venv::setup(py)?;

            let module = py_venv::embed_module(
                py,
                c_str!("def greet(): return 'hello world!'"),
                c"hello.py",
                c"hello",
            )?;

            let kwargs = PyDict::new(py);
            let result = py_venv::call_kwargs(py, &module, "greet", &kwargs)?;
            let message: String = result.extract()?;

            Ok::<_, pyo3::PyErr>(message)
        })
    })
    .await
    .expect("spawn_blocking panicked")
    .expect("Python error");

    // Back in async context — GIL is released
    println!("Python said: {result}");

    // Can continue awaiting other things
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    println!("Done.");
}

use wasm_bindgen::prelude::*;

#[wasm_bindgen(getter_with_clone)]
pub struct Sentence {
    pub text: String,
    pub paragraph_end: bool,
}

#[wasm_bindgen]
pub fn split(text: &str) -> Vec<Sentence> {
    splitter::split(text)
        .into_iter()
        .map(|s| Sentence {
            text: s.text,
            paragraph_end: s.paragraph_end,
        })
        .collect()
}

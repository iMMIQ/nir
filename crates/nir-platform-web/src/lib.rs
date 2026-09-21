//! Browser primitives; no narrative or application policy.
#![forbid(unsafe_code)]
#[cfg(target_arch = "wasm32")]
pub fn canvas(id: &str) -> std::result::Result<web_sys::HtmlCanvasElement, wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("E_CANVAS: missing canvas"))?
        .dyn_into()
        .map_err(|_| wasm_bindgen::JsValue::from_str("E_CANVAS: expected canvas"))
}

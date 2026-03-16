/// Erkennungs-Bibliothek für GPU-Bibliotheken in Projektverzeichnissen.
/// Wird vom Wizard (Phase 4b) genutzt.
/// Phase 1: Nur Grundstruktur, vollständige Implementierung in Phase 4b.
pub struct DetectionResult {
    pub gpu_libraries: Vec<GpuLibrary>,
    pub workload_types: Vec<String>,
    pub compose_services: Vec<String>,
}

pub struct GpuLibrary {
    pub name: String,
    pub version: Option<String>,
    pub workload_type: String,
}

/// Bekannte GPU-Bibliotheken → Workload-Typ Mapping
pub fn known_gpu_libraries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("torch", "llm"),
        ("tensorflow", "training"),
        ("jax", "training"),
        ("sentence-transformers", "embeddings"),
        ("faiss-gpu", "embeddings"),
        ("donut-python", "ocr"),
        ("pytesseract", "ocr"),
        ("onnxruntime-gpu", "inference"),
        ("llama-cpp-python", "llm"),
        ("ollama", "llm"),
        ("pgvector", "embeddings"),
        ("transformers", "llm"),
        ("accelerate", "training"),
        ("easyocr", "ocr"),
        ("chandra-ocr", "ocr"),
    ]
}

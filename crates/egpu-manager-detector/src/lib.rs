/// Erkennungs-Bibliothek für GPU-Bibliotheken in Projektverzeichnissen.
/// Scannt requirements.txt, pyproject.toml, Cargo.toml, package.json und
/// docker-compose.yml nach GPU-relevanten Abhängigkeiten.

use std::path::Path;

pub struct DetectionResult {
    pub gpu_libraries: Vec<GpuLibrary>,
    pub workload_types: Vec<String>,
    pub compose_services: Vec<String>,
    pub has_gpu_usage: bool,
}

pub struct GpuLibrary {
    pub name: String,
    pub version: Option<String>,
    pub workload_type: String,
    pub source: String,
}

/// Bekannte GPU-Bibliotheken → Workload-Typ Mapping
pub fn known_gpu_libraries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("torch", "llm"),
        ("tensorflow", "training"),
        ("tensorflow-gpu", "training"),
        ("jax", "training"),
        ("jaxlib", "training"),
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
        ("paddleocr", "ocr"),
        ("paddlepaddle-gpu", "ocr"),
        ("whisper", "transcription"),
        ("openai-whisper", "transcription"),
        ("faster-whisper", "transcription"),
        ("cupy", "compute"),
        ("cudf", "compute"),
        ("rapids", "compute"),
        ("pycuda", "compute"),
        ("numba", "compute"),
        ("triton", "inference"),
        ("vllm", "llm"),
        ("tgi", "llm"),
        ("diffusers", "image_generation"),
        ("comfyui", "image_generation"),
    ]
}

/// Scannt ein Projektverzeichnis nach GPU-Bibliotheken und Workload-Typen.
pub fn detect(project_path: &Path) -> DetectionResult {
    let mut result = DetectionResult {
        gpu_libraries: Vec::new(),
        workload_types: Vec::new(),
        compose_services: Vec::new(),
        has_gpu_usage: false,
    };

    let known = known_gpu_libraries();

    // Scan requirements.txt
    for req_file in &["requirements.txt", "requirements-gpu.txt", "requirements-cuda.txt"] {
        let path = project_path.join(req_file);
        if let Ok(content) = std::fs::read_to_string(&path) {
            scan_requirements(&content, &known, req_file, &mut result);
        }
    }

    // Scan pyproject.toml
    let pyproject = project_path.join("pyproject.toml");
    if let Ok(content) = std::fs::read_to_string(&pyproject) {
        scan_pyproject(&content, &known, &mut result);
    }

    // Scan Cargo.toml (Rust projects)
    let cargo_toml = project_path.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        scan_cargo_toml(&content, &known, &mut result);
    }

    // Scan package.json (Node projects)
    let package_json = project_path.join("package.json");
    if let Ok(content) = std::fs::read_to_string(&package_json) {
        scan_package_json(&content, &known, &mut result);
    }

    // Scan docker-compose.yml
    for compose_name in &[
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ] {
        let path = project_path.join(compose_name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            scan_compose(&content, &mut result);
            break;
        }
    }

    // Also check backend/ subdirectory
    let backend = project_path.join("backend");
    if backend.is_dir() {
        for req_file in &["requirements.txt", "requirements-gpu.txt"] {
            let path = backend.join(req_file);
            if let Ok(content) = std::fs::read_to_string(&path) {
                scan_requirements(&content, &known, &format!("backend/{req_file}"), &mut result);
            }
        }
        let pyproject = backend.join("pyproject.toml");
        if let Ok(content) = std::fs::read_to_string(&pyproject) {
            scan_pyproject(&content, &known, &mut result);
        }
    }

    // Deduplicate workload types
    result.workload_types.sort();
    result.workload_types.dedup();
    result.has_gpu_usage = !result.gpu_libraries.is_empty();

    result
}

fn scan_requirements(
    content: &str,
    known: &[(&str, &str)],
    source: &str,
    result: &mut DetectionResult,
) {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Extract package name (before ==, >=, ~=, [, etc.)
        let pkg = line
            .split(&['=', '>', '<', '~', '[', ';', ' '][..])
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();

        for (name, wtype) in known {
            if pkg == *name || pkg.replace('-', "_") == name.replace('-', "_") {
                let version = line
                    .split("==")
                    .nth(1)
                    .or_else(|| line.split(">=").nth(1))
                    .map(|v| v.trim().to_string());

                result.gpu_libraries.push(GpuLibrary {
                    name: name.to_string(),
                    version,
                    workload_type: wtype.to_string(),
                    source: source.to_string(),
                });
                if !result.workload_types.contains(&wtype.to_string()) {
                    result.workload_types.push(wtype.to_string());
                }
            }
        }
    }
}

fn scan_pyproject(content: &str, known: &[(&str, &str)], result: &mut DetectionResult) {
    // Simple string search — no TOML parsing needed
    let content_lower = content.to_lowercase();
    for (name, wtype) in known {
        if content_lower.contains(&name.to_lowercase()) {
            if !result.gpu_libraries.iter().any(|g| g.name == *name) {
                result.gpu_libraries.push(GpuLibrary {
                    name: name.to_string(),
                    version: None,
                    workload_type: wtype.to_string(),
                    source: "pyproject.toml".to_string(),
                });
                if !result.workload_types.contains(&wtype.to_string()) {
                    result.workload_types.push(wtype.to_string());
                }
            }
        }
    }
}

fn scan_cargo_toml(content: &str, _known: &[(&str, &str)], result: &mut DetectionResult) {
    let content_lower = content.to_lowercase();
    let rust_gpu_crates = [
        ("cuda-sys", "compute"),
        ("cust", "compute"),
        ("cudarc", "compute"),
        ("wgpu", "compute"),
        ("vulkano", "compute"),
        ("candle", "llm"),
        ("burn", "training"),
        ("tch", "llm"),
    ];

    for (name, wtype) in &rust_gpu_crates {
        if content_lower.contains(name) {
            result.gpu_libraries.push(GpuLibrary {
                name: name.to_string(),
                version: None,
                workload_type: wtype.to_string(),
                source: "Cargo.toml".to_string(),
            });
            if !result.workload_types.contains(&wtype.to_string()) {
                result.workload_types.push(wtype.to_string());
            }
        }
    }
}

fn scan_package_json(content: &str, _known: &[(&str, &str)], result: &mut DetectionResult) {
    let content_lower = content.to_lowercase();
    let node_gpu_pkgs = [
        ("@xenova/transformers", "llm"),
        ("onnxruntime-node", "inference"),
        ("tensorflow", "training"),
    ];

    for (name, wtype) in &node_gpu_pkgs {
        if content_lower.contains(name) {
            result.gpu_libraries.push(GpuLibrary {
                name: name.to_string(),
                version: None,
                workload_type: wtype.to_string(),
                source: "package.json".to_string(),
            });
            if !result.workload_types.contains(&wtype.to_string()) {
                result.workload_types.push(wtype.to_string());
            }
        }
    }
}

fn scan_compose(content: &str, result: &mut DetectionResult) {
    let content_lower = content.to_lowercase();

    // Detect GPU usage markers
    let has_nvidia = content_lower.contains("runtime: nvidia")
        || content_lower.contains("nvidia_visible_devices")
        || content_lower.contains("capabilities: [gpu]");

    if has_nvidia {
        result.has_gpu_usage = true;
    }

    // Extract service names
    let mut in_services = false;
    let mut base_indent = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "services:" || trimmed.starts_with("services:") {
            in_services = true;
            base_indent = line.len() - line.trim_start().len();
            continue;
        }
        if in_services && !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent = line.len() - line.trim_start().len();
            if indent == base_indent + 2 && trimmed.ends_with(':') {
                let name = trimmed.trim_end_matches(':').to_string();
                result.compose_services.push(name);
            }
            if indent <= base_indent && !trimmed.is_empty() && trimmed != "services:" {
                in_services = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_gpu_libraries_not_empty() {
        assert!(!known_gpu_libraries().is_empty());
    }

    #[test]
    fn test_scan_requirements() {
        let content = "torch==2.1.0\nnumpy>=1.24\nsentence-transformers>=2.0\n";
        let known = known_gpu_libraries();
        let mut result = DetectionResult {
            gpu_libraries: Vec::new(),
            workload_types: Vec::new(),
            compose_services: Vec::new(),
            has_gpu_usage: false,
        };
        scan_requirements(content, &known, "requirements.txt", &mut result);
        assert_eq!(result.gpu_libraries.len(), 2);
        assert!(result.workload_types.contains(&"llm".to_string()));
        assert!(result.workload_types.contains(&"embeddings".to_string()));
    }

    #[test]
    fn test_scan_compose_nvidia() {
        let content = "services:\n  worker:\n    runtime: nvidia\n    image: myapp\n  redis:\n    image: redis\n";
        let mut result = DetectionResult {
            gpu_libraries: Vec::new(),
            workload_types: Vec::new(),
            compose_services: Vec::new(),
            has_gpu_usage: false,
        };
        scan_compose(content, &mut result);
        assert!(result.has_gpu_usage);
        assert_eq!(result.compose_services, vec!["worker", "redis"]);
    }

    #[test]
    fn test_scan_compose_no_gpu() {
        let content = "services:\n  web:\n    image: nginx\n";
        let mut result = DetectionResult {
            gpu_libraries: Vec::new(),
            workload_types: Vec::new(),
            compose_services: Vec::new(),
            has_gpu_usage: false,
        };
        scan_compose(content, &mut result);
        assert!(!result.has_gpu_usage);
        assert_eq!(result.compose_services, vec!["web"]);
    }

    #[test]
    fn test_scan_requirements_with_comments() {
        let content = "# GPU deps\ntorch>=2.0\n# not gpu\nflask==3.0\n";
        let known = known_gpu_libraries();
        let mut result = DetectionResult {
            gpu_libraries: Vec::new(),
            workload_types: Vec::new(),
            compose_services: Vec::new(),
            has_gpu_usage: false,
        };
        scan_requirements(content, &known, "requirements.txt", &mut result);
        assert_eq!(result.gpu_libraries.len(), 1);
        assert_eq!(result.gpu_libraries[0].name, "torch");
    }
}

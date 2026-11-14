use super::ProviderSpec;

/// NVIDIA NIM (https://integrate.api.nvidia.com/v1)
pub struct NvidiaNim;

/// Substrings of model ids that are not chat models (embeddings, rerankers, detectors, ...).
const NON_CHAT_MARKERS: &[&str] = &["embed", "reward", "detector", "calibration", "nvclip"];

impl ProviderSpec for NvidiaNim {
    fn name(&self) -> &'static str {
        "NVIDIA NIM"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["nvidia", "nvidianim", "nim"]
    }

    fn default_base_url(&self) -> &'static str {
        "https://integrate.api.nvidia.com/v1"
    }

    fn is_chat_model(&self, model_id: &str) -> bool {
        let id = model_id.to_lowercase();
        !NON_CHAT_MARKERS.iter().any(|marker| id.contains(marker))
    }
}

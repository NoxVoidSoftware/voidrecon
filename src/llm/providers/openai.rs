use super::ProviderSpec;

/// OpenAI direct API
pub struct OpenAi;

impl ProviderSpec for OpenAi {
    fn name(&self) -> &'static str {
        "OpenAI"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["openai"]
    }

    fn default_base_url(&self) -> &'static str {
        "https://api.openai.com/v1"
    }
}

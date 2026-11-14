use super::ProviderSpec;

/// OpenRouter (https://openrouter.ai)
pub struct OpenRouter;

impl ProviderSpec for OpenRouter {
    fn name(&self) -> &'static str {
        "OpenRouter"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["openrouter"]
    }

    fn default_base_url(&self) -> &'static str {
        "https://openrouter.ai/api/v1"
    }

    /// App attribution headers recognised by OpenRouter.
    fn extra_headers(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("HTTP-Referer", "https://github.com/NoxVoid/voidrecon"),
            ("X-Title", "voidrecon"),
        ]
    }
}

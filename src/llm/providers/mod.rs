//! LLM provider registry for voidrecon.
//!
//! Every provider speaks the OpenAI-compatible chat API, so a provider is just
//! a [`ProviderSpec`]: a static description of its name, endpoint, auth policy
//! and quirks. The rest of the app only talks to [`ProviderKind`] (the
//! serializable id) and [`Provider`] (a spec bound to a URL and key).
//!
//! # Adding a provider
//!
//! 1. Create `providers/<name>.rs` with a unit struct implementing [`ProviderSpec`].
//! 2. Add one line to the `providers!` list below.
//!
//! Nothing else needs to change: the setup modal, the model picker, the
//! `:provider` command and the HTTP client are all driven by the registry.

use serde::{Deserialize, Serialize};

/// Whether a provider needs, accepts, or ignores an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyPolicy {
    /// Requests fail without a key; the user is prompted for one.
    Required,
    /// A key is sent when configured (self-hosted endpoints behind auth).
    Optional,
    /// No key is ever sent.
    Unused,
}

/// Static description of an OpenAI-compatible LLM provider.
pub trait ProviderSpec: Send + Sync + 'static {
    /// Display name. Also part of persisted context-limit keys, so renaming
    /// an existing provider forgets its learned context limits.
    fn name(&self) -> &'static str;

    /// Names accepted by `:provider <alias>`; the first one is shown in help.
    fn aliases(&self) -> &'static [&'static str];

    fn default_base_url(&self) -> &'static str;

    fn api_key_policy(&self) -> ApiKeyPolicy {
        ApiKeyPolicy::Required
    }

    /// Whether `:provider <alias> <url>` may point the provider at another
    /// endpoint (local or self-hosted servers).
    fn accepts_custom_url(&self) -> bool {
        false
    }

    /// Extra headers sent with every chat request.
    fn extra_headers(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// Filters the provider's model list down to chat-capable models.
    fn is_chat_model(&self, _model_id: &str) -> bool {
        true
    }
}

/// Declares the provider modules, the [`ProviderKind`] enum, [`ALL_PROVIDERS`]
/// and the kind → spec lookup from a single list. The first entry is the default.
macro_rules! providers {
    ($($variant:ident => $module:ident :: $spec:ident),+ $(,)?) => {
        $(mod $module;)+

        /// Identifier of a supported provider, as stored in `config.json`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum ProviderKind {
            $($variant,)+
        }

        /// Every supported provider, in the order shown in the UI.
        pub const ALL_PROVIDERS: &[ProviderKind] = &[$(ProviderKind::$variant,)+];

        impl ProviderKind {
            pub fn spec(self) -> &'static dyn ProviderSpec {
                match self {
                    $(ProviderKind::$variant => &$module::$spec,)+
                }
            }
        }
    };
}

providers! {
    OpenRouter => openrouter::OpenRouter,
    NvidiaNim => nvidia::NvidiaNim,
    OpenAi => openai::OpenAi,
    Ollama => ollama::Ollama,
    Custom => custom::Custom,
}

impl Default for ProviderKind {
    fn default() -> Self {
        ALL_PROVIDERS[0]
    }
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        self.spec().name()
    }

    pub fn default_base_url(&self) -> &'static str {
        self.spec().default_base_url()
    }

    pub fn requires_api_key(&self) -> bool {
        self.spec().api_key_policy() == ApiKeyPolicy::Required
    }

    /// Resolves a `:provider` command argument (case-insensitive).
    pub fn from_alias(alias: &str) -> Option<Self> {
        ALL_PROVIDERS.iter().copied().find(|kind| {
            kind.spec()
                .aliases()
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(alias))
        })
    }
}

/// A provider bound to a concrete endpoint and API key.
#[derive(Clone)]
pub struct Provider {
    kind: ProviderKind,
    base_url: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The API key is deliberately left out of debug output.
        f.debug_struct("Provider")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("has_api_key", &self.api_key.is_some())
            .finish()
    }
}

impl Provider {
    pub fn from_config(kind: ProviderKind, api_key: &str, custom_base_url: Option<String>) -> Self {
        let spec = kind.spec();
        let api_key = api_key.trim();
        let api_key = match spec.api_key_policy() {
            ApiKeyPolicy::Unused => None,
            ApiKeyPolicy::Required | ApiKeyPolicy::Optional if api_key.is_empty() => None,
            ApiKeyPolicy::Required | ApiKeyPolicy::Optional => Some(api_key.to_string()),
        };
        Self {
            kind,
            base_url: custom_base_url.unwrap_or_else(|| spec.default_base_url().to_string()),
            api_key,
        }
    }

    pub fn name(&self) -> &'static str {
        self.kind.as_str()
    }

    /// The key to send as a bearer token, if this provider uses one and it is set.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn requires_api_key(&self) -> bool {
        self.kind.requires_api_key()
    }

    pub fn extra_headers(&self) -> &'static [(&'static str, &'static str)] {
        self.kind.spec().extra_headers()
    }

    pub fn is_chat_model(&self, model_id: &str) -> bool {
        self.kind.spec().is_chat_model(model_id)
    }

    pub fn chat_endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    pub fn models_endpoint(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_entries_are_well_formed() {
        let mut names = HashSet::new();
        let mut aliases = HashSet::new();
        for kind in ALL_PROVIDERS {
            let spec = kind.spec();
            assert!(names.insert(spec.name()), "duplicate name {}", spec.name());
            assert!(!spec.aliases().is_empty(), "{} has no alias", spec.name());
            for alias in spec.aliases() {
                assert!(aliases.insert(*alias), "duplicate alias {alias}");
                assert_eq!(ProviderKind::from_alias(alias), Some(*kind));
            }
            assert!(spec.default_base_url().starts_with("http"));
        }
    }

    #[test]
    fn config_ids_stay_stable() {
        // These strings are persisted in config.json.
        let ids: Vec<String> = ALL_PROVIDERS
            .iter()
            .map(|kind| serde_json::to_string(kind).unwrap())
            .collect();
        assert_eq!(
            ids,
            ["\"open_router\"", "\"nvidia_nim\"", "\"open_ai\"", "\"ollama\"", "\"custom\""]
        );
        assert_eq!(ProviderKind::default(), ProviderKind::OpenRouter);
    }

    #[test]
    fn api_key_follows_provider_policy() {
        let key = |kind| Provider::from_config(kind, " secret ", None).api_key().map(str::to_string);
        assert_eq!(key(ProviderKind::OpenAi).as_deref(), Some("secret"));
        assert_eq!(key(ProviderKind::Custom).as_deref(), Some("secret"));
        assert_eq!(key(ProviderKind::Ollama), None);
        assert_eq!(Provider::from_config(ProviderKind::Custom, "  ", None).api_key(), None);
    }

    #[test]
    fn endpoints_use_custom_base_url() {
        let provider = Provider::from_config(
            ProviderKind::Ollama,
            "",
            Some("http://10.0.0.5:11434/v1/".to_string()),
        );
        assert_eq!(provider.chat_endpoint(), "http://10.0.0.5:11434/v1/chat/completions");
        assert_eq!(provider.models_endpoint(), "http://10.0.0.5:11434/v1/models");
        assert_eq!(
            Provider::from_config(ProviderKind::OpenAi, "k", None).base_url,
            "https://api.openai.com/v1"
        );
    }
}

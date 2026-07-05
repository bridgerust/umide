//! Model catalog + cost-aware routing.
//!
//! The point: don't burn the user's credits. A simple "explain this" must not go
//! to the flagship (Opus 4.8 / Fable 5). Each provider exposes capability/cost
//! TIERS, and [`route_tier`] picks the cheapest tier that fits the turn —
//! reserving the deep model for agentic, multi-step work. The app can override
//! the choice (a manual model pick); this module only provides the catalog and
//! the default Auto policy.

use crate::provider::ProviderKind;

/// A capability/cost tier. `Fast` is cheapest, `Deep` is the most capable
/// everyday model, `Flagship` is the top model (opt-in only — never the Auto
/// default). `Balanced` is the sensible default for anything not clearly simple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Fast,
    Balanced,
    Deep,
    Flagship,
}

impl ProviderKind {
    /// The concrete model ID for a tier.
    ///
    /// Claude + OpenAI (the v1 providers) have current per-tier IDs. DeepSeek
    /// and Gemini aren't in the v1 set; they map to their single default so the
    /// match stays total.
    pub fn model_for_tier(self, tier: ModelTier) -> &'static str {
        use ModelTier::*;
        use ProviderKind::*;
        match (self, tier) {
            // Claude — current catalog.
            (Anthropic, Fast) => "claude-haiku-4-5-20251001",
            (Anthropic, Balanced) => "claude-sonnet-5",
            (Anthropic, Deep) => "claude-opus-4-8",
            (Anthropic, Flagship) => "claude-fable-5",

            // OpenAI — current catalog (2026): GPT-5.4-mini for fast/cheap
            // turns, GPT-5.5 for the strong agentic coder, GPT-5.5-pro flagship.
            // Balanced == Deep for now (both GPT-5.5); a deeper split can later
            // ride `reasoning.effort` rather than a separate model. IDs are from
            // the OpenAI docs — validate against a live call when a key is
            // exercised end-to-end.
            (OpenAi, Fast) => "gpt-5.4-mini",
            (OpenAi, Balanced) => "gpt-5.5",
            (OpenAi, Deep) => "gpt-5.5",
            (OpenAi, Flagship) => "gpt-5.5-pro",

            // Not in the v1 provider set — single default keeps the match total.
            (DeepSeek, _) => "deepseek-chat",
            (Gemini, _) => "gemini-2.0-flash",
        }
    }

    /// Whether this provider has a real per-tier catalog (Claude + OpenAI in
    /// v1). Lets the router avoid pretending to tier a provider whose tiers all
    /// resolve to the same ID (DeepSeek / Gemini).
    pub fn has_tiered_models(self) -> bool {
        matches!(self, ProviderKind::Anthropic | ProviderKind::OpenAi)
    }
}

/// What we know about a turn, for routing. Cheap to compute from the request;
/// no model call needed.
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteSignals {
    /// The turn is expected to drive the agent loop — tool use, edits, shell
    /// commands, the see→act→verify device loop, multi-step work. Deep tier.
    pub agentic: bool,
    /// Length of the user's message in characters. A short prompt with no
    /// agentic intent is treated as a simple Q&A → Fast tier.
    pub user_chars: usize,
}

/// A short prompt below this length, with no agentic intent, is "simple".
const SIMPLE_CHARS: usize = 240;

/// The default Auto policy: cheapest tier that fits.
/// - agentic turn → `Deep`
/// - short, non-agentic → `Fast`
/// - everything else → `Balanced`
///
/// Never returns `Flagship` — that's opt-in via a manual pick only.
pub fn route_tier(signals: RouteSignals) -> ModelTier {
    if signals.agentic {
        ModelTier::Deep
    } else if signals.user_chars <= SIMPLE_CHARS {
        ModelTier::Fast
    } else {
        ModelTier::Balanced
    }
}

/// Convenience: the model ID Auto would use for `provider` on this turn. If the
/// provider isn't tiered yet, always its default model (no false tiering).
pub fn auto_model(provider: ProviderKind, signals: RouteSignals) -> &'static str {
    if provider.has_tiered_models() {
        provider.model_for_tier(route_tier(signals))
    } else {
        provider.default_model()
    }
}

/// Cheap heuristic: does this user turn look like it will drive the agent loop
/// (edits, shell commands, the run-on-device loop) rather than a plain
/// question/explanation? Imperative action verbs ⇒ agentic → the Deep tier;
/// otherwise it's treated as a simple turn. Deliberately biases action requests
/// toward Deep (better to spend on a real task than route it to a weak model).
pub fn looks_agentic(user_text: &str) -> bool {
    let t = user_text.to_lowercase();
    const ACTION: &[&str] = &[
        "implement",
        "fix",
        "add ",
        "change",
        "modify",
        "refactor",
        "build",
        "run ",
        "create",
        "write",
        "install",
        "debug",
        "rename",
        "delete",
        "remove",
        "move ",
        "generate",
        "replace",
        "wire",
        "integrate",
        "set up",
        "make ",
        "apply",
        "commit",
        "test",
        "deploy",
    ];
    ACTION.iter().any(|k| t.contains(k))
}

/// Build routing signals from a user turn.
pub fn signals_for(user_text: &str) -> RouteSignals {
    RouteSignals {
        agentic: looks_agentic(user_text),
        user_chars: user_text.chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ProviderKind::*;

    #[test]
    fn claude_tiers_are_distinct_and_ordered() {
        assert_eq!(
            Anthropic.model_for_tier(ModelTier::Fast),
            "claude-haiku-4-5-20251001"
        );
        assert_eq!(
            Anthropic.model_for_tier(ModelTier::Balanced),
            "claude-sonnet-5"
        );
        assert_eq!(Anthropic.model_for_tier(ModelTier::Deep), "claude-opus-4-8");
        assert_eq!(
            Anthropic.model_for_tier(ModelTier::Flagship),
            "claude-fable-5"
        );
        // All four are different models.
        let ids = [
            ModelTier::Fast,
            ModelTier::Balanced,
            ModelTier::Deep,
            ModelTier::Flagship,
        ]
        .map(|t| Anthropic.model_for_tier(t));
        let mut uniq = ids.to_vec();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 4);
    }

    #[test]
    fn auto_never_picks_flagship_and_saves_on_simple_turns() {
        // Simple short question → Fast (cheapest), NOT Opus/Fable.
        let simple = RouteSignals {
            agentic: false,
            user_chars: 30,
        };
        assert_eq!(route_tier(simple), ModelTier::Fast);
        assert_eq!(auto_model(Anthropic, simple), "claude-haiku-4-5-20251001");

        // Agentic turn (run on device, edits) → Deep.
        let agentic = RouteSignals {
            agentic: true,
            user_chars: 30,
        };
        assert_eq!(route_tier(agentic), ModelTier::Deep);
        assert_eq!(auto_model(Anthropic, agentic), "claude-opus-4-8");

        // Long but non-agentic → Balanced (Sonnet), not the flagship.
        let longish = RouteSignals {
            agentic: false,
            user_chars: 900,
        };
        assert_eq!(route_tier(longish), ModelTier::Balanced);

        // Auto must never return Fable across the signal space.
        for agentic in [false, true] {
            for user_chars in [0usize, 100, 300, 5000] {
                let m = auto_model(
                    Anthropic,
                    RouteSignals {
                        agentic,
                        user_chars,
                    },
                );
                assert_ne!(m, "claude-fable-5", "Auto must never pick the flagship");
            }
        }
    }

    #[test]
    fn signals_classify_questions_vs_actions() {
        // Plain questions → not agentic → cheap.
        assert!(!looks_agentic("what does this function do?"));
        assert!(!looks_agentic("explain how navigation works here"));
        assert_eq!(
            auto_model(Anthropic, signals_for("what does this do?")),
            "claude-haiku-4-5-20251001"
        );
        // Action requests → agentic → Deep.
        assert!(looks_agentic(
            "add a dark-mode toggle and run it on the device"
        ));
        assert!(looks_agentic("fix the crash on the login screen"));
        assert_eq!(
            auto_model(Anthropic, signals_for("implement the settings screen")),
            "claude-opus-4-8"
        );
    }

    #[test]
    fn openai_is_tiered_and_saves_on_simple_turns() {
        assert!(OpenAi.has_tiered_models());
        // Simple question → the cheap mini, not GPT-5.5.
        assert_eq!(
            auto_model(OpenAi, signals_for("what does this do?")),
            "gpt-5.4-mini"
        );
        // Action request → the strong coder.
        assert_eq!(
            auto_model(OpenAi, signals_for("implement the login flow")),
            "gpt-5.5"
        );
        // Auto never picks the pro flagship.
        for agentic in [false, true] {
            for user_chars in [0usize, 300, 5000] {
                assert_ne!(
                    auto_model(
                        OpenAi,
                        RouteSignals {
                            agentic,
                            user_chars
                        }
                    ),
                    "gpt-5.5-pro"
                );
            }
        }
    }

    #[test]
    fn untiered_provider_uses_its_default_not_false_tiers() {
        // DeepSeek isn't tiered → Auto returns its default, no pretend tiering.
        assert!(!DeepSeek.has_tiered_models());
        let agentic = RouteSignals {
            agentic: true,
            user_chars: 10,
        };
        assert_eq!(auto_model(DeepSeek, agentic), DeepSeek.default_model());
    }
}

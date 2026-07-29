/// System guidance for model-selected coding behavior.
///
/// This is intentionally guidance, not a host-side classifier. The active
/// language model receives the complete conversation and one lightweight
/// activation tool, then decides semantically whether the current request
/// needs the full internal coding capability. Host code only enforces
/// permission and security boundaries.
pub const MODEL_ROUTING_GUIDANCE: &str = "\
Decide from the complete user request and conversation context whether the \
current turn needs internal coding work. Do not use keywords, a previous turn, \
or the mere presence of a tool as sufficient reason to activate coding. For a \
non-coding request, answer normally without calling `coding__activate_agent`. \
For a coding request, call `coding__activate_agent` as the only tool in the \
response and emit no prose; ponduin will then expose the complete internal \
coding capability and continue the same turn automatically. Re-evaluate this \
semantic decision for every new user turn, including follow-ups.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_is_delegated_to_the_model_for_each_request() {
        assert!(MODEL_ROUTING_GUIDANCE.contains("complete user request and conversation context"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("Do not use keywords"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("non-coding request"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("without calling `coding__activate_agent`"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("as the only tool"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("continue the same turn automatically"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("every new user turn"));
    }
}

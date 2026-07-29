/// System guidance for model-selected coding behavior.
///
/// This is intentionally guidance, not a host-side classifier. The active
/// language model receives the complete conversation and the internal tools,
/// then decides semantically whether the current request needs coding work.
/// Host code only enforces permission and security boundaries.
pub const MODEL_ROUTING_GUIDANCE: &str = "\
Decide from the complete user request and conversation context whether the \
current turn needs internal coding work. Do not use keywords, a previous turn, \
or the mere presence of tools as sufficient reason to call them. For a \
non-coding request, answer normally without calling `coding__` tools. For a \
coding request, choose the fitting working approach yourself: implement \
requested behavior in small validated patches; debug from evidence and a \
falsifiable hypothesis; refactor with behavior-preserving checks; analyze \
repositories read-only unless changes are requested; generate behavior-focused \
tests using repository conventions; document only verified behavior; and \
review without editing by default. Re-evaluate the need and approach for every \
new user turn.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_is_delegated_to_the_model_for_each_request() {
        assert!(MODEL_ROUTING_GUIDANCE.contains("complete user request and conversation context"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("Do not use keywords"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("non-coding request"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("without calling `coding__` tools"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("choose the fitting working approach yourself"));
        assert!(MODEL_ROUTING_GUIDANCE.contains("every new user turn"));
    }
}

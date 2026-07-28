use rand::seq::IndexedRandom;

/// Clear, professional status messages shown while Ponduin is working.
const THINKING_MESSAGES: &[&str] = &[
    "Analyzing the request",
    "Reviewing context",
    "Planning the next step",
    "Evaluating options",
    "Processing information",
    "Inspecting dependencies",
    "Checking assumptions",
    "Exploring solution paths",
    "Validating the approach",
    "Preparing the response",
    "Coordinating tools",
    "Reviewing results",
    "Verifying consistency",
    "Finalizing the result",
];

/// Returns a random status message
pub fn get_random_thinking_message() -> &'static str {
    THINKING_MESSAGES
        .choose(&mut rand::rng())
        .unwrap_or(&THINKING_MESSAGES[0])
}

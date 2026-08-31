use codex_extension_api::TURN_COMPLETION_CONTRIBUTION_MAX_BYTES;
use codex_extension_api::TurnCompletionContribution;
use pretty_assertions::assert_eq;

#[test]
fn turn_completion_contribution_accepts_content_at_the_byte_limit() {
    let text = "x".repeat(TURN_COMPLETION_CONTRIBUTION_MAX_BYTES);

    let contribution =
        TurnCompletionContribution::new(text.clone()).expect("content at the limit is valid");

    assert_eq!(contribution.as_str(), text);
}

#[test]
fn turn_completion_contribution_rejects_content_above_the_byte_limit() {
    let text = "x".repeat(TURN_COMPLETION_CONTRIBUTION_MAX_BYTES + 1);

    let error = TurnCompletionContribution::new(text).expect_err("oversize content is invalid");

    assert_eq!(
        error.actual_bytes(),
        TURN_COMPLETION_CONTRIBUTION_MAX_BYTES + 1
    );
    assert_eq!(error.max_bytes(), TURN_COMPLETION_CONTRIBUTION_MAX_BYTES);
}

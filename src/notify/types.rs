//! Private notification state; credentials are never serialized or debug-printed.

pub(super) struct Slack {
    pub(super) token: String,
    pub(super) channel_id: String,
    pub(super) review_message: Option<String>,
    pub(super) deploy_message: Option<String>,
    pub(super) agent: ureq::Agent,
    pub(super) endpoint: String,
}

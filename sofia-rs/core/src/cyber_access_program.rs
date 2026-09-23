use sofia_api::AccessPrograms;
use sofia_login::CodexAuth;
use sofia_protocol::turn_input::CyberAccessProgram;

pub(crate) fn for_auth(
    auth: Option<&CodexAuth>,
    program: Option<CyberAccessProgram>,
) -> Option<AccessPrograms> {
    program
        .filter(|_| auth.is_some_and(CodexAuth::is_chatgpt_auth))
        .map(AccessPrograms::from)
}

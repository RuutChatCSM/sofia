#[cfg(sofia_bazel)]
pub use code_mode_proto::sofia::code_mode::v1::*;

#[cfg(not(sofia_bazel))]
tonic::include_proto!("sofia.code_mode.v1");

pub const MAX_IDENTIFIER_BYTES: usize = 256;

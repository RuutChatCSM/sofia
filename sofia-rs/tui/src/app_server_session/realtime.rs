//! App-server signaling for TUI-owned realtime WebRTC sessions.

use super::AppServerSession;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use sofia_app_server_protocol::ClientRequest;
use sofia_app_server_protocol::ThreadRealtimeAppendSpeechParams;
use sofia_app_server_protocol::ThreadRealtimeAppendSpeechResponse;
use sofia_app_server_protocol::ThreadRealtimeStartParams;
use sofia_app_server_protocol::ThreadRealtimeStartResponse;
use sofia_app_server_protocol::ThreadRealtimeStartTransport;
use sofia_app_server_protocol::ThreadRealtimeStopParams;
use sofia_app_server_protocol::ThreadRealtimeStopResponse;
use sofia_protocol::ThreadId;
use sofia_protocol::protocol::RealtimeConversationVersion;
use sofia_protocol::protocol::RealtimeOutputModality;

impl AppServerSession {
    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        offer_sdp: String,
        model: Option<String>,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStart {
                request_id,
                params: ThreadRealtimeStartParams {
                    thread_id: thread_id.to_string(),
                    client_managed_handoffs: Some(true),
                    delegation_ack_filler: None,
                    flush_transcript_tail_on_session_end: None,
                    sofia_responses_as_items: None,
                    sofia_response_item_prefix: None,
                    sofia_response_handoff_mode: None,
                    sofia_response_handoff_channel_prefixes: None,
                    model,
                    output_modality: RealtimeOutputModality::Audio,
                    include_startup_context: Some(false),
                    initial_items: None,
                    realtime_start_instructions: None,
                    realtime_end_instructions: None,
                    prompt: None,
                    realtime_session_id: None,
                    transport: Some(ThreadRealtimeStartTransport::Webrtc { sdp: offer_sdp }),
                    version: Some(RealtimeConversationVersion::V3),
                    voice: None,
                },
            })
            .await
            .wrap_err("thread/realtime/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStopResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStop {
                request_id,
                params: ThreadRealtimeStopParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/realtime/stop failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_append_speech(
        &mut self,
        thread_id: ThreadId,
        text: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendSpeechResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendSpeech {
                request_id,
                params: ThreadRealtimeAppendSpeechParams {
                    thread_id: thread_id.to_string(),
                    text,
                },
            })
            .await
            .wrap_err("thread/realtime/appendSpeech failed in TUI")?;
        Ok(())
    }
}

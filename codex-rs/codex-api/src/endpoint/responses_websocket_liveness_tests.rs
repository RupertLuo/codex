use super::*;

const IDLE: Duration = Duration::from_millis(50);

async fn assert_stream_liveness(messages: Vec<Message>, delay: Duration) -> Result<(), ApiError> {
    let (tx_command, mut rx_command) = mpsc::channel(1);
    let (tx_message, rx_message) = mpsc::unbounded_channel();
    let pump_task = tokio::spawn(async move {
        while let Some(WsCommand::Send { tx_result, .. }) = rx_command.recv().await {
            let _ = tx_result.send(Ok(()));
        }
    });
    let mut stream = WsStream {
        tx_command,
        rx_message,
        pump_task,
    };
    let producer = tokio::spawn(async move {
        for message in messages {
            tokio::time::sleep(delay).await;
            if tx_message.send(Ok(message)).is_err() {
                return;
            }
        }
        // Keep the peer open after the last event; EOF must not rescue the test.
        tx_message.closed().await;
    });
    let (tx_event, mut rx_event) = mpsc::channel(16);
    let consumer = tokio::spawn(async move { while rx_event.recv().await.is_some() {} });
    let context = ResponsesWebsocketTimingLogContext {
        model: "fixture".into(),
        session_id: None,
        thread_id: None,
        turn_id: None,
        traceparent: None,
        previous_response_id: None,
        request_start_ms: None,
        warmup: false,
        connection_reused: false,
    };
    let result = run_websocket_response_stream(
        &mut stream,
        tx_event,
        "{}".into(),
        IDLE,
        /*telemetry*/ None,
        /*turn_state*/ None,
        &context,
    )
    .await;
    producer.abort();
    consumer.await.expect("consumer");
    result
}

#[tokio::test(start_paused = true)]
async fn non_progress_text_events_do_not_extend_websocket_deadline() {
    for text in [
        "ping",
        r#"{"type":"ping"}"#,
        r#"{"type":"response.in_progress"}"#,
    ] {
        let messages = vec![Message::Text(text.into()); 20];
        let result = tokio::time::timeout(
            IDLE * 2,
            assert_stream_liveness(messages, Duration::from_millis(10)),
        )
        .await;
        assert!(
            matches!(result, Ok(Err(ApiError::Stream(message))) if message.contains("timeout"))
        );
    }
}

#[tokio::test(start_paused = true)]
async fn tool_argument_progress_keeps_websocket_alive() {
    let mut messages = vec![
        Message::Text(
            r#"{"type":"response.function_call_arguments.delta","delta":"x"}"#.into()
        );
        20
    ];
    messages.push(Message::Text(
        r#"{"type":"response.completed","response":{"id":"done"}}"#.into(),
    ));
    assert!(
        assert_stream_liveness(messages, Duration::from_millis(10))
            .await
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn websocket_terminal_error_does_not_wait_for_peer_close() {
    let messages = vec![Message::Text(
        r#"{"type":"response.failed","response":{"error":{"code":"context_length_exceeded"}}}"#
            .into(),
    )];
    let result = tokio::time::timeout(
        Duration::from_millis(1),
        assert_stream_liveness(messages, Duration::ZERO),
    )
    .await;
    assert!(matches!(result, Ok(Err(ApiError::ContextWindowExceeded))));
}

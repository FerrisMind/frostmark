//! Bridge LLM / fixture streaming into iced `Subscription` messages.

use std::hash::{Hash, Hasher};
use std::time::Duration;

use futures::StreamExt;
use iced::Subscription;
use iced::futures::SinkExt;
use iced::stream;
use serde::Deserialize;

use super::openai_compat::{ApiConfig, ChatMessage};

#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    Delta { msg_id: u64, chunk: String },
    Done,
    Error { msg_id: u64, error: String },
}

#[derive(Debug, Clone)]
pub enum ActiveStream {
    Api {
        msg_id: u64,
        config: ApiConfig,
        messages: Vec<ChatMessage>,
    },
    Simulate {
        msg_id: u64,
        chunks: Vec<String>,
    },
}

impl Hash for ActiveStream {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            ActiveStream::Api { msg_id, .. } | ActiveStream::Simulate { msg_id, .. } => {
                msg_id.hash(state);
            }
        }
    }
}

impl Hash for ApiConfig {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.base_url.hash(state);
        self.api_key.hash(state);
        self.model.hash(state);
    }
}

pub fn stream_subscription(active: &Option<ActiveStream>) -> Subscription<ChatStreamEvent> {
    let Some(active) = active.clone() else {
        return Subscription::none();
    };
    Subscription::run_with(active, run_worker)
}

fn run_worker(active: &ActiveStream) -> impl iced::futures::Stream<Item = ChatStreamEvent> + use<> {
    let active = active.clone();
    stream::channel(256, async move |mut output| match active {
        ActiveStream::Api {
            msg_id,
            config,
            messages,
        } => match stream_api(&config, &messages, msg_id, &mut output).await {
            Ok(()) => {
                let _ = output.send(ChatStreamEvent::Done).await;
            }
            Err(error) => {
                let _ = output.send(ChatStreamEvent::Error { msg_id, error }).await;
            }
        },
        ActiveStream::Simulate { msg_id, chunks } => {
            for chunk in chunks {
                tokio::time::sleep(Duration::from_millis(25)).await;
                if output
                    .send(ChatStreamEvent::Delta { msg_id, chunk })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            let _ = output.send(ChatStreamEvent::Done).await;
        }
    })
}

async fn stream_api(
    config: &ApiConfig,
    messages: &[ChatMessage],
    msg_id: u64,
    output: &mut iced::futures::channel::mpsc::Sender<ChatStreamEvent>,
) -> Result<(), String> {
    let base = config.base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");

    let body = serde_json::json!({
        "model": config.model,
        "messages": messages,
        "stream": true,
    });

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {text}"));
    }

    let mut byte_stream = response.bytes_stream();
    let mut buffer = Vec::new();

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        for line in decode_sse_lines(&mut buffer, &chunk)? {
            if let Some(delta) = parse_sse_delta(&line)
                && output
                    .send(ChatStreamEvent::Delta {
                        msg_id,
                        chunk: delta,
                    })
                    .await
                    .is_err()
            {
                return Ok(());
            }
        }
    }

    if !buffer.is_empty() {
        let line = String::from_utf8(buffer)
            .map_err(|error| format!("invalid UTF-8 in SSE stream: {error}"))?;
        if let Some(delta) = parse_sse_delta(line.trim())
            && output
                .send(ChatStreamEvent::Delta {
                    msg_id,
                    chunk: delta,
                })
                .await
                .is_err()
        {
            return Ok(());
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

fn parse_sse_delta(line: &str) -> Option<String> {
    let payload = line
        .strip_prefix("data:")
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    if payload == "[DONE]" {
        return None;
    }

    let chunk: StreamChunk = serde_json::from_str(payload).ok()?;
    chunk
        .choices
        .first()
        .and_then(|c| c.delta.content.clone())
        .filter(|s| !s.is_empty())
}

fn decode_sse_lines(buffer: &mut Vec<u8>, chunk: &[u8]) -> Result<Vec<String>, String> {
    buffer.extend_from_slice(chunk);
    let mut lines = Vec::new();

    while let Some(position) = buffer.iter().position(|byte| *byte == b'\n') {
        let line = buffer.drain(..=position).collect::<Vec<_>>();
        let line = String::from_utf8(line)
            .map_err(|error| format!("invalid UTF-8 in SSE stream: {error}"))?;
        lines.push(line.trim_end_matches('\n').trim().to_owned());
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_preserves_utf8_split_across_chunks() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"Привет\"}}]}\n";
        let split = line.find('П').expect("Cyrillic payload") + 1;
        let mut buffer = Vec::new();

        assert!(
            decode_sse_lines(&mut buffer, &line.as_bytes()[..split])
                .expect("first chunk")
                .is_empty()
        );
        let lines = decode_sse_lines(&mut buffer, &line.as_bytes()[split..]).expect("second chunk");

        assert_eq!(lines, vec![line.trim_end_matches('\n').to_owned()]);
        assert_eq!(parse_sse_delta(&lines[0]).as_deref(), Some("Привет"));
    }
}

/// Word/whitespace chunks that preserve newlines (needed for GFM tables in TEST.md).
pub fn chunk_by_words(text: &str, words_per_chunk: usize) -> Vec<String> {
    let words_per_chunk = words_per_chunk.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut words_in_chunk = 0usize;
    let mut i = 0usize;

    while i < text.len() {
        let rest = &text[i..];
        if let Some(ws) = rest.chars().next().filter(|c| c.is_whitespace()) {
            let len = ws.len_utf8()
                + rest
                    .chars()
                    .skip(1)
                    .take_while(|c| c.is_whitespace())
                    .map(|c| c.len_utf8())
                    .sum::<usize>();
            current.push_str(&text[i..i + len]);
            i += len;
            continue;
        }

        let word_len = rest
            .chars()
            .take_while(|c| !c.is_whitespace())
            .map(|c| c.len_utf8())
            .sum::<usize>();
        current.push_str(&text[i..i + word_len]);
        i += word_len;
        words_in_chunk += 1;

        if words_in_chunk >= words_per_chunk {
            chunks.push(std::mem::take(&mut current));
            words_in_chunk = 0;
        }
    }

    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

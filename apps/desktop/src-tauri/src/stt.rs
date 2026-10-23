use std::{collections::HashMap, sync::Mutex};

#[derive(Default)]
pub struct VoiceRequests(Mutex<HashMap<String, tokio::task::AbortHandle>>);

fn audio_format(mime_type: &str) -> Result<(&'static str, &'static str), String> {
    match mime_type.split(';').next().unwrap_or("").trim() {
        "audio/mp4" => Ok(("audio/mp4", "voice.m4a")),
        "audio/webm" => Ok(("audio/webm", "voice.webm")),
        _ => Err("unsupported".into()),
    }
}

pub(crate) fn audio_form(
    audio: Vec<u8>,
    mime_type: &str,
    language: &str,
) -> Result<reqwest::multipart::Form, String> {
    let (mime, filename) = audio_format(mime_type)?;
    if audio.is_empty() {
        return Err("empty_audio".into());
    }
    if audio.len() > 10 * 1024 * 1024 {
        return Err("too_large".into());
    }
    if !matches!(language, "" | "ko" | "en") {
        return Err("language".into());
    }
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(filename)
        .mime_str(mime)
        .map_err(|_| "unsupported".to_owned())?;
    let form = reqwest::multipart::Form::new().part("file", part);
    Ok(if language.is_empty() {
        form
    } else {
        form.text("language", language.to_owned())
    })
}

#[tauri::command]
pub async fn transcribe_voice(
    requests: tauri::State<'_, VoiceRequests>,
    request_id: String,
    audio: Vec<u8>,
    mime_type: String,
    language: String,
) -> Result<String, String> {
    if !crate::auth::is_signed_in() {
        return Err("auth".into());
    }
    if uuid::Uuid::parse_str(&request_id).is_err() {
        return Err("request".into());
    }
    // Validate before spawning a request, including its memory limit.
    audio_form(audio.clone(), &mime_type, &language)?;
    let task = {
        let mut pending = requests.0.lock().map_err(|_| "busy".to_owned())?;
        if pending.len() >= 4 || pending.contains_key(&request_id) {
            return Err("busy".into());
        }
        let task = tauri::async_runtime::spawn(async move {
            crate::auth::transcribe_voice_audio(audio, &mime_type, &language).await
        });
        pending.insert(request_id.clone(), task.inner().abort_handle());
        task
    };
    let result = task.await.map_err(|_| "cancelled".to_owned());
    if let Ok(mut pending) = requests.0.lock() {
        pending.remove(&request_id);
    }
    result?
}

#[tauri::command]
pub fn cancel_voice_transcription(requests: tauri::State<'_, VoiceRequests>, request_id: String) {
    if let Ok(pending) = requests.0.lock() {
        if let Some(task) = pending.get(&request_id) {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_browser_audio_and_optional_language() {
        for mime in [
            "audio/mp4",
            "audio/mp4;codecs=mp4a.40.2",
            "audio/webm;codecs=opus",
        ] {
            for language in ["", "ko", "en"] {
                assert!(audio_form(vec![1, 2], mime, language).is_ok());
            }
        }
    }

    #[test]
    fn rejects_empty_oversize_or_unsupported_uploads() {
        assert!(audio_form(vec![], "audio/mp4", "ko").is_err());
        assert!(audio_form(vec![0; 10 * 1024 * 1024 + 1], "audio/mp4", "ko").is_err());
        assert!(audio_form(vec![1], "video/mp4", "ko").is_err());
        assert!(audio_form(vec![1], "audio/mp4", "../ko").is_err());
    }
}

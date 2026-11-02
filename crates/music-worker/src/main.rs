//! Um processo por reprodução. Configuração JSON na primeira linha de stdin;
//! EOF cancela. stdout contém apenas eventos JSON; nunca imprimir o JWT.
use livekit::{
    options::{TrackPublishOptions, audio::MUSIC_STEREO},
    prelude::*,
    webrtc::{
        audio_source::{AudioSourceOptions, RtcAudioSource, native::NativeAudioSource},
        prelude::AudioFrame,
    },
};
use protocol::{MusicWorkerEvent, MusicWorkerJob};
use std::{borrow::Cow, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{Instant, timeout},
};

const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const FRAME_SAMPLES: usize = 960;
const FRAME_BYTES: usize = FRAME_SAMPLES * CHANNELS as usize * 2;

#[tokio::main]
async fn main() {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut line = vec![];
    let read = timeout(
        Duration::from_secs(10),
        (&mut stdin).take(16_385).read_until(b'\n', &mut line),
    )
    .await;
    if !matches!(read, Ok(Ok(n)) if n > 0 && n <= 16_384) {
        let _ = emit(MusicWorkerEvent::Failed {
            reason: "Configuração ausente ou muito grande.".into(),
        })
        .await;
        return;
    }
    let job: MusicWorkerJob = match serde_json::from_slice(&line) {
        Ok(job) => job,
        Err(_) => {
            let _ = emit(MusicWorkerEvent::Failed {
                reason: "Configuração inválida.".into(),
            })
            .await;
            return;
        }
    };
    if !valid_job(&job) {
        let _ = emit(MusicWorkerEvent::Failed {
            reason: "Faixa ou posição inválida.".into(),
        })
        .await;
        return;
    }
    // Pai mantém stdin aberto. Qualquer outra linha ou EOF significa cancelar.
    let mut control = [0u8; 1];
    tokio::select! {
        result = play(job) => {
            let event = match result { Ok(event) => event, Err(reason) => MusicWorkerEvent::Failed { reason } };
            let _ = emit(event).await;
        }
        _ = stdin.read(&mut control) => {}
    }
}

fn valid_job(job: &MusicWorkerJob) -> bool {
    job.video_id.len() == 11
        && job
            .video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && job.duration_ms > 0
        && job.duration_ms <= 1_200_000
        && job.position_ms < job.duration_ms
        && (job.url.starts_with("wss://") || job.url.starts_with("ws://"))
        && !job.token.is_empty()
}

async fn emit(event: MusicWorkerEvent) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(&event).map_err(|_| "Evento inválido.")?;
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(&bytes)
        .await
        .map_err(|_| "Servidor desconectou.")?;
    stdout
        .flush()
        .await
        .map_err(|_| "Servidor desconectou.".into())
}

async fn play(job: MusicWorkerJob) -> Result<MusicWorkerEvent, String> {
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (room, mut events) = timeout(
        Duration::from_secs(15),
        Room::connect(&job.url, &job.token, options),
    )
    .await
    .map_err(|_| "LiveKit demorou para conectar.")?
    .map_err(|_| "Não foi possível conectar ao LiveKit.")?;
    let source = NativeAudioSource::new(
        AudioSourceOptions {
            echo_cancellation: false,
            noise_suppression: false,
            auto_gain_control: false,
        },
        RATE,
        CHANNELS,
        100,
    );
    let track = LocalTrack::Audio(LocalAudioTrack::create_audio_track(
        "mothh-music",
        RtcAudioSource::Native(source.clone()),
    ));
    room.local_participant()
        .publish_track(
            track,
            TrackPublishOptions {
                source: TrackSource::Microphone,
                audio_encoding: Some(MUSIC_STEREO.encoding),
                ..Default::default()
            },
        )
        .await
        .map_err(|_| "Falha ao publicar música.")?;

    // Não consumir a fila sem ouvintes. A inscrição pode ocorrer depois do pedido.
    let listener = timeout(Duration::from_secs(30), async {
        loop {
            if !room.remote_participants().is_empty() {
                return Ok(());
            }
            match events.recv().await {
                None | Some(RoomEvent::Disconnected { .. }) => {
                    return Err("Conexão LiveKit encerrada.".to_string());
                }
                _ => {}
            }
        }
    })
    .await;
    match listener {
        Err(_) => {
            let _ = room.close().await;
            return Ok(MusicWorkerEvent::Empty);
        }
        Ok(Err(reason)) => return Err(reason),
        Ok(Ok(())) => {}
    }

    let yt_dlp = std::env::var_os("MOTHH_YT_DLP").unwrap_or_else(|| "yt-dlp".into());
    let ffmpeg = std::env::var_os("MOTHH_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
    let url = format!("https://www.youtube.com/watch?v={}", job.video_id);
    let mut extractor = Command::new(yt_dlp)
        .args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-playlist",
            "--no-progress",
            "--no-warnings",
            "--socket-timeout",
            "10",
            "--retries",
            "0",
            "--format",
            "bestaudio",
            "--output",
            "-",
            "--",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Não foi possível iniciar yt-dlp.")?;
    let extractor_diagnostic = tokio::spawn(read_diagnostic(
        extractor.stderr.take().ok_or("Extrator sem diagnóstico.")?,
    ));
    let offset = format!("{:.3}", job.position_ms as f64 / 1000.0);
    let remaining = format!("{:.3}", (job.duration_ms - job.position_ms) as f64 / 1000.0);
    // Decoder recebe bytes por pipe, nunca URL controlada por usuário.
    // -ss depois de -i descarta o início até a posição; retomar não é instantâneo.
    let mut decoder = Command::new(ffmpeg)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-protocol_whitelist",
            "pipe",
            "-i",
            "pipe:0",
            "-ss",
            &offset,
            "-t",
            &remaining,
            "-vn",
            "-threads",
            "1",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Não foi possível iniciar FFmpeg.")?;
    let decoder_diagnostic = tokio::spawn(read_diagnostic(
        decoder.stderr.take().ok_or("Decoder sem diagnóstico.")?,
    ));
    let encoded = extractor.stdout.take().ok_or("Extrator sem saída.")?;
    let decoder_input = decoder.stdin.take().ok_or("Decoder sem entrada.")?;
    let mut pcm = decoder.stdout.take().ok_or("Decoder sem saída.")?;
    // copy é limitado pelo pipe do sistema; sem arquivo temporário ou buffer crescente.
    let pump = copy_media(encoded, decoder_input);
    tokio::pin!(pump);
    let playback = async {
        let mut buffer = [0u8; FRAME_BYTES];
        let mut frames = 0u64;
        let mut empty_since = None;
        let mut next = Instant::now();
        loop {
            if room.remote_participants().is_empty() {
                let since = empty_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(30) {
                    return Ok(MusicWorkerEvent::Empty);
                }
            } else {
                empty_since = None;
            }
            // read_exact trata o fim parcial da última janela sem perder amostras.
            let mut filled = 0;
            while filled < FRAME_BYTES {
                let read = timeout(Duration::from_secs(30), pcm.read(&mut buffer[filled..]))
                    .await
                    .map_err(|_| "Fonte parou de enviar áudio.")?
                    .map_err(|_| "Falha na decodificação.")?;
                if read == 0 {
                    break;
                }
                filled += read;
            }
            if filled == 0 {
                break;
            }
            if filled % (CHANNELS as usize * 2) != 0 {
                return Err("Quadro PCM incompleto.".into());
            }
            if frames == 0 {
                emit(MusicWorkerEvent::Playing).await?;
                next = Instant::now();
            }
            tokio::time::sleep_until(next).await;
            next += Duration::from_millis(20);
            // Não acelerar em rajada caso a origem tenha travado.
            if next < Instant::now() {
                next = Instant::now();
            }
            let data = buffer[..filled]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect::<Vec<_>>();
            let frame = AudioFrame {
                data: Cow::Owned(data),
                sample_rate: RATE,
                num_channels: CHANNELS,
                samples_per_channel: (filled / 4) as u32,
            };
            source
                .capture_frame(&frame)
                .await
                .map_err(|_| "Falha ao enviar quadro musical.")?;
            frames += (filled / 4) as u64;
            if frames % 48_000 < FRAME_SAMPLES as u64 {
                emit(MusicWorkerEvent::Progress {
                    position_ms: job.position_ms + frames * 1000 / u64::from(RATE),
                })
                .await?;
            }
        }
        if frames == 0 {
            return Err("A fonte não retornou áudio.".into());
        }
        let played_ms = frames * 1000 / u64::from(RATE);
        if played_ms + 2500 < job.duration_ms - job.position_ms {
            return Err("A fonte encerrou antes do fim da música.".into());
        }
        Ok(MusicWorkerEvent::Finished)
    };
    tokio::pin!(playback);
    let mut pump_done = false;
    let result: Result<MusicWorkerEvent, String> = loop {
        tokio::select! {
            result = &mut playback => break result,
            event = events.recv() => {
                if matches!(event, None | Some(RoomEvent::Disconnected { .. })) { break Err("Conexão LiveKit encerrada.".into()); }
            }
            result = &mut pump, if !pump_done => {
                if result.is_err() { break Err("Falha ao transferir mídia ao decoder.".into()); }
                pump_done = true;
            }
        }
    };
    // Conferir término do decoder: EOF por erro não é fim natural.
    let result = if matches!(result, Ok(MusicWorkerEvent::Finished)) {
        match timeout(Duration::from_secs(3), decoder.wait()).await {
            Ok(Ok(status)) if status.success() => result,
            _ => Err("Decoder encerrou com erro.".into()),
        }
    } else {
        result
    };
    let extractor_status = extractor.try_wait().ok().flatten();
    let decoder_status = decoder.try_wait().ok().flatten();
    let _ = extractor.kill().await;
    let _ = decoder.kill().await;
    let extractor_error = diagnostic_result(extractor_diagnostic).await;
    let decoder_error = diagnostic_result(decoder_diagnostic).await;
    let _ = room.close().await;
    result.map_err(|reason| {
        format!(
            "{reason} yt-dlp: {} / {}; FFmpeg: {} / {}.",
            extractor_status.map_or_else(|| "ainda ativo".into(), |s| s.to_string()),
            classify_diagnostic(&extractor_error),
            decoder_status.map_or_else(|| "ainda ativo".into(), |s| s.to_string()),
            classify_diagnostic(&decoder_error),
        )
    })
}

async fn copy_media(
    mut source: impl tokio::io::AsyncRead + Unpin,
    mut destination: impl tokio::io::AsyncWrite + Unpin,
) -> std::io::Result<()> {
    let result = tokio::io::copy(&mut source, &mut destination).await;
    // ChildStdin::shutdown não garante fechar o descritor do pipe.
    // Sem drop, FFmpeg pode esperar mais bytes em vez de produzir EOF.
    drop(destination);
    result.map(|_| ())
}

// Drenar stderr em paralelo evita bloquear os processos. Guardar só os últimos
// 8 KiB; conteúdo bruto (URLs assinadas, cookies etc.) nunca sai do worker.
async fn read_diagnostic(mut input: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut tail = Vec::new();
    let mut buffer = [0u8; 1024];
    while let Ok(n) = input.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        let excess = (tail.len() + n).saturating_sub(8192);
        tail.drain(..excess);
        tail.extend_from_slice(&buffer[..n]);
    }
    tail
}

async fn diagnostic_result(mut task: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    match timeout(Duration::from_secs(1), &mut task).await {
        Ok(Ok(bytes)) => bytes,
        _ => {
            task.abort();
            Vec::new()
        }
    }
}

fn classify_diagnostic(bytes: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(bytes).to_lowercase();
    if text.contains("sign in") || text.contains("not a bot") || text.contains("login required") {
        "YouTube exigiu autenticação/verificação"
    } else if text.contains("403") || text.contains("forbidden") {
        "acesso à mídia recusado (403)"
    } else if text.contains("429") || text.contains("too many requests") {
        "limite de requisições (429)"
    } else if text.contains("challenge")
        || text.contains("javascript")
        || text.contains("js runtime")
    {
        "falha no runtime JavaScript/desafio do provedor"
    } else if text.contains("requested format") {
        "formato de áudio indisponível"
    } else if text.contains("invalid data") || text.contains("error opening input") {
        "entrada de áudio inválida ou vazia"
    } else if text.contains("timed out") || text.contains("timeout") {
        "tempo limite de rede"
    } else if text.contains("unavailable") || text.contains("not available") {
        "vídeo indisponível ou restrito"
    } else if bytes.is_empty() {
        "sem diagnóstico"
    } else {
        "diagnóstico não classificado"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn diagnostics_are_bounded_and_do_not_expose_source_secrets() {
        let bytes = read_diagnostic(tokio::io::repeat(b'x').take(20_000)).await;
        assert_eq!(bytes.len(), 8192);
        assert_eq!(
            classify_diagnostic(b"ERROR HTTP 403 https://host/?token=secret"),
            "acesso à mídia recusado (403)"
        );
        assert_eq!(
            classify_diagnostic(b"Sign in to confirm you are not a bot"),
            "YouTube exigiu autenticação/verificação"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn completed_copy_closes_child_stdin_even_while_future_is_retained() {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pump = copy_media(&b"audio fixture"[..], child.stdin.take().unwrap());
        tokio::pin!(pump);
        (&mut pump).await.unwrap();
        let mut output = Vec::new();
        timeout(
            Duration::from_secs(2),
            child.stdout.take().unwrap().read_to_end(&mut output),
        )
        .await
        .expect("child must receive EOF before the pump future is dropped")
        .unwrap();
        assert_eq!(output, b"audio fixture");
        assert!(child.wait().await.unwrap().success());
    }

    #[test]
    fn job_rejects_invalid_source_and_positions() {
        let mut job = MusicWorkerJob {
            url: "ws://127.0.0.1:7880".into(),
            token: "test".into(),
            video_id: "abcdefghijk".into(),
            position_ms: 0,
            duration_ms: 1000,
        };
        assert!(valid_job(&job));
        job.position_ms = 1000;
        assert!(!valid_job(&job));
        job.position_ms = 0;
        job.video_id = "../anything".into();
        assert!(!valid_job(&job));
    }
}

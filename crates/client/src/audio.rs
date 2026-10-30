//! Mensagens de áudio: gravar do microfone, codificar em Opus e tocar.
//!
//! O som roda numa thread só dele. Os streams do `cpal` não podem trocar de
//! thread em todo sistema, e no Windows o microfone precisa de uma
//! configuração (COM) que briga com a da janela; numa thread separada os dois
//! não se encontram. A interface e a rede falam com ela por um `AudioHandle`.
//!
//! O arquivo é Opus dentro de Ogg, mono, 48 kHz, a 24 kbit/s: voz clara e uns
//! 3 KB por segundo.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use opus_pure::{
    Application, MAX_PACKET_BYTES, MAX_PACKET_SAMPLES, OggOpusReader, OggOpusWriter, OpusEncoder,
    OpusHead, Trim,
};
use protocol::MAX_AUDIO_MS;
use slint::SharedString;
use tokio::sync::mpsc::UnboundedSender;

use crate::{AppWindow, Command};

/// Taxa de amostragem do arquivo. O Opus trabalha em 48 kHz por dentro.
const RATE: u32 = 48_000;
/// 20 ms por pacote, o tamanho mais comum para voz.
const FRAME: usize = (RATE / 50) as usize;
const BITRATE: i32 = 24_000;
/// Soltar o botão antes disso não manda nada: foi um clique sem querer.
const MIN_RECORDING: Duration = Duration::from_millis(500);
/// De quanto em quanto tempo o relógio da gravação e do player anda na tela.
const TICK: Duration = Duration::from_millis(100);

/// O que a interface e a rede pedem para a thread de som.
enum Request {
    StartRecording,
    /// Termina e manda o áudio para a rede.
    StopRecording,
    /// Termina e joga fora.
    CancelRecording,
    /// Toca o arquivo; `id` é o do anexo, para a tela saber qual está tocando.
    Play {
        id: String,
        ogg: Vec<u8>,
    },
    StopPlayback,
}

/// Controle remoto da thread de som. Clonar é barato.
#[derive(Clone)]
pub struct AudioHandle(Sender<Request>);

impl AudioHandle {
    pub fn start_recording(&self) {
        let _ = self.0.send(Request::StartRecording);
    }
    pub fn stop_recording(&self) {
        let _ = self.0.send(Request::StopRecording);
    }
    pub fn cancel_recording(&self) {
        let _ = self.0.send(Request::CancelRecording);
    }
    pub fn play(&self, id: String, ogg: Vec<u8>) {
        let _ = self.0.send(Request::Play { id, ogg });
    }
    pub fn stop_playback(&self) {
        let _ = self.0.send(Request::StopPlayback);
    }
}

/// Abre a thread de som. O áudio gravado vai para a rede como
/// `Command::SendAudio`.
pub fn spawn(ui: slint::Weak<AppWindow>, net: UnboundedSender<Command>) -> AudioHandle {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(rx, ui, net));
    AudioHandle(tx)
}

/// Uma gravação em andamento. Largar o `stream` desliga o microfone.
struct Recording {
    _stream: Stream,
    /// Amostras mono, na taxa do microfone (`rate`).
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    started: Instant,
}

/// Um áudio tocando. Largar o `stream` para o som.
struct Playback {
    _stream: Stream,
    id: String,
    /// Quantos quadros o alto-falante já tocou, de `total`.
    played: Arc<AtomicUsize>,
    total: usize,
    rate: u32,
}

fn run(requests: Receiver<Request>, ui: slint::Weak<AppWindow>, net: UnboundedSender<Command>) {
    let mut recording: Option<Recording> = None;
    let mut playback: Option<Playback> = None;
    // O player da tela está mostrando um áudio; ao parar, limpa uma vez só.
    let mut showing_playback = false;

    loop {
        // Sem pedido novo, acorda a cada `TICK` para atualizar os relógios.
        let request = match requests.recv_timeout(TICK) {
            Ok(request) => Some(request),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };

        match request {
            Some(Request::StartRecording) if recording.is_none() => {
                // Gravar por cima de um áudio tocando ia gravar o próprio som.
                playback = None;
                match start_recording() {
                    Ok(started) => {
                        recording = Some(started);
                        show_recording(&ui, Some(Duration::ZERO));
                    }
                    Err(reason) => show_status(&ui, reason),
                }
            }
            Some(Request::StartRecording) => {}
            Some(Request::StopRecording) => {
                if let Some(done) = recording.take() {
                    show_recording(&ui, None);
                    finish_recording(done, &ui, &net);
                }
            }
            Some(Request::CancelRecording) => {
                if recording.take().is_some() {
                    show_recording(&ui, None);
                    show_status(&ui, "gravação cancelada".into());
                }
            }
            Some(Request::Play { id, ogg }) => {
                if recording.is_some() {
                    continue;
                }
                playback = None;
                match start_playback(id, &ogg) {
                    Ok(started) => playback = Some(started),
                    Err(reason) => show_status(&ui, reason),
                }
            }
            Some(Request::StopPlayback) => playback = None,
            None => {}
        }

        if let Some(current) = &recording {
            let elapsed = current.started.elapsed();
            if elapsed >= Duration::from_millis(MAX_AUDIO_MS.into()) {
                // Chegou no limite do servidor: manda o que já tem.
                show_recording(&ui, None);
                if let Some(done) = recording.take() {
                    finish_recording(done, &ui, &net);
                }
            } else {
                show_recording(&ui, Some(elapsed));
            }
        }
        if playback
            .as_ref()
            .is_some_and(|p| p.played.load(Ordering::Relaxed) >= p.total)
        {
            playback = None;
        }
        match &playback {
            Some(current) => {
                show_playback(&ui, Some(current));
                showing_playback = true;
            }
            None if showing_playback => {
                show_playback(&ui, None);
                showing_playback = false;
            }
            None => {}
        }
    }
}

/// Liga o microfone padrão e começa a guardar o som.
fn start_recording() -> Result<Recording, String> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or("nenhum microfone encontrado")?;
    let supported = device
        .default_input_config()
        .map_err(|err| format!("o microfone não abriu: {err}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let samples = Arc::new(Mutex::new(Vec::new()));

    let stream = match format {
        SampleFormat::F32 => input_stream::<f32>(&device, &config, samples.clone()),
        SampleFormat::I16 => input_stream::<i16>(&device, &config, samples.clone()),
        SampleFormat::I32 => input_stream::<i32>(&device, &config, samples.clone()),
        SampleFormat::U16 => input_stream::<u16>(&device, &config, samples.clone()),
        other => {
            return Err(format!(
                "o microfone usa um formato que não sei ler ({other})"
            ));
        }
    }
    .map_err(|err| format!("o microfone não abriu: {err}"))?;
    stream
        .play()
        .map_err(|err| format!("o microfone não ligou: {err}"))?;

    Ok(Recording {
        _stream: stream,
        samples,
        rate: config.sample_rate.0,
        started: Instant::now(),
    })
}

/// Stream de entrada que mistura os canais em mono e guarda em `samples`.
fn input_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    // Passou do limite do servidor, para de guardar (a thread de som já vai
    // encerrar a gravação).
    let max = config.sample_rate.0 as usize * MAX_AUDIO_MS as usize / 1000;
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let Ok(mut samples) = samples.lock() else {
                return;
            };
            for frame in data.chunks(channels) {
                if samples.len() >= max {
                    return;
                }
                let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                samples.push(sum / channels as f32);
            }
        },
        |err| eprintln!("erro no microfone: {err}"),
        None,
    )
}

/// Desliga o microfone, codifica e entrega o arquivo para a rede.
fn finish_recording(done: Recording, ui: &slint::Weak<AppWindow>, net: &UnboundedSender<Command>) {
    let Recording {
        _stream,
        samples,
        rate,
        started,
    } = done;
    drop(_stream);
    if started.elapsed() < MIN_RECORDING {
        show_status(ui, "segure o mic para gravar".into());
        return;
    }
    let samples = std::mem::take(&mut *samples.lock().unwrap_or_else(|e| e.into_inner()));
    let pcm = resample(&samples, rate, RATE);
    match encode(&pcm) {
        Ok(ogg) => {
            let _ = net.send(Command::SendAudio(ogg));
        }
        Err(err) => show_status(ui, format!("não deu para codificar o áudio: {err}")),
    }
}

/// Decodifica o arquivo e toca no alto-falante padrão.
fn start_playback(id: String, ogg: &[u8]) -> Result<Playback, String> {
    let pcm = decode(ogg).map_err(|err| format!("esse áudio não abriu: {err}"))?;
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("nenhum alto-falante encontrado")?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("o alto-falante não abriu: {err}"))?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let pcm = Arc::new(resample(&pcm, RATE, config.sample_rate.0));
    let played = Arc::new(AtomicUsize::new(0));

    let stream = match format {
        SampleFormat::F32 => output_stream::<f32>(&device, &config, pcm.clone(), played.clone()),
        SampleFormat::I16 => output_stream::<i16>(&device, &config, pcm.clone(), played.clone()),
        SampleFormat::I32 => output_stream::<i32>(&device, &config, pcm.clone(), played.clone()),
        SampleFormat::U16 => output_stream::<u16>(&device, &config, pcm.clone(), played.clone()),
        other => {
            return Err(format!(
                "o alto-falante usa um formato que não sei tocar ({other})"
            ));
        }
    }
    .map_err(|err| format!("o alto-falante não abriu: {err}"))?;
    stream
        .play()
        .map_err(|err| format!("o alto-falante não ligou: {err}"))?;

    Ok(Playback {
        _stream: stream,
        id,
        played,
        total: pcm.len(),
        rate: config.sample_rate.0,
    })
}

/// Stream de saída que toca `pcm` (mono) igual em todos os canais.
fn output_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    pcm: Arc<Vec<f32>>,
    played: Arc<AtomicUsize>,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let mut position = played.load(Ordering::Relaxed);
            for frame in data.chunks_mut(channels) {
                // Depois do fim, silêncio até a thread de som fechar o stream.
                let value = pcm.get(position).copied().unwrap_or(0.0);
                frame.fill(T::from_sample(value));
                position += 1;
            }
            played.store(position.min(pcm.len()), Ordering::Relaxed);
        },
        |err| eprintln!("erro no alto-falante: {err}"),
        None,
    )
}

/// Codifica áudio mono a 48 kHz em Ogg Opus.
///
/// O codificador fica alguns milissegundos atrás da entrada, então no fim
/// entra um pouco de silêncio para empurrar o resto para fora, e o último
/// pacote diz onde o áudio de verdade acaba. Assim a duração do arquivo é a
/// da gravação, nem mais nem menos.
pub fn encode(pcm: &[f32]) -> opus_pure::Result<Vec<u8>> {
    let mut encoder = OpusEncoder::new(RATE as i32, 1, Application::Voip)?;
    encoder.bitrate_bps = BITRATE;
    let head = OpusHead::for_encoder(&encoder, RATE);
    let frames = (pcm.len() + usize::from(head.pre_skip)).div_ceil(FRAME);
    let final_granule = u64::from(head.pre_skip) + pcm.len() as u64;

    let mut writer = OggOpusWriter::new(Vec::new(), head)?;
    let mut packet = vec![0u8; MAX_PACKET_BYTES];
    let mut block = [0f32; FRAME];
    for i in 0..frames {
        let start = (i * FRAME).min(pcm.len());
        let end = (start + FRAME).min(pcm.len());
        block[..end - start].copy_from_slice(&pcm[start..end]);
        block[end - start..].fill(0.0);
        let n = encoder.encode(&block, FRAME, &mut packet)?;
        if i + 1 == frames {
            let duration = final_granule.saturating_sub(writer.granule() as u64);
            writer.write_packet_with_duration(&packet[..n], duration as u32)?;
        } else {
            writer.write_packet(&packet[..n])?;
        }
    }
    writer.finish()
}

/// Decodifica um Ogg Opus para mono a 48 kHz (estéreo vira a média dos dois).
pub fn decode(ogg: &[u8]) -> opus_pure::Result<Vec<f32>> {
    let mut reader = OggOpusReader::new(ogg)?;
    let head = reader.head().clone();
    let channels = usize::from(head.channel_count.max(1));
    let mut decoder = head.decoder(RATE as i32)?;
    let mut trim = Trim::new(&head, RATE as i32, channels)?;
    let mut block = vec![0f32; MAX_PACKET_SAMPLES * channels];
    let mut pcm = Vec::new();
    for packet in reader.packets() {
        let packet = packet?;
        let n = decoder.decode(&packet.data, MAX_PACKET_SAMPLES, &mut block)?;
        for frame in trim.keep(&packet, &block[..n * channels]).chunks(channels) {
            pcm.push(frame.iter().sum::<f32>() / channels as f32);
        }
    }
    Ok(pcm)
}

/// Muda a taxa de amostragem por interpolação linear. Simples, e para voz
/// (44,1 kHz do microfone para 48 kHz do Opus, por exemplo) basta.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 || to == 0 {
        return samples.to_vec();
    }
    let len = (samples.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..len)
        .map(|i| {
            let pos = i as f64 * step;
            let index = pos as usize;
            let frac = (pos - index as f64) as f32;
            let a = samples[index.min(samples.len() - 1)];
            let b = samples[(index + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// "0:10", "1:05": a duração como aparece no player.
pub fn mm_ss(ms: u64) -> String {
    let secs = (ms + 500) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn show_status(ui: &slint::Weak<AppWindow>, status: String) {
    let _ = ui.upgrade_in_event_loop(move |ui| ui.set_status(status.into()));
}

/// `Some(tempo)` enquanto grava; `None` some com o aviso.
fn show_recording(ui: &slint::Weak<AppWindow>, elapsed: Option<Duration>) {
    let text: SharedString = elapsed
        .map(|e| mm_ss(e.as_millis() as u64 / 1000 * 1000))
        .unwrap_or_default()
        .into();
    let on = elapsed.is_some();
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_recording(on);
        ui.set_recording_time(text);
    });
}

/// Qual áudio está tocando e onde ele está, para o player da mensagem.
fn show_playback(ui: &slint::Weak<AppWindow>, current: Option<&Playback>) {
    let (id, progress, time): (SharedString, f32, SharedString) = match current {
        Some(p) => {
            let played = p.played.load(Ordering::Relaxed).min(p.total);
            let ms = played as u64 * 1000 / u64::from(p.rate.max(1));
            (
                p.id.clone().into(),
                played as f32 / p.total.max(1) as f32,
                mm_ss(ms / 1000 * 1000).into(),
            )
        }
        None => Default::default(),
    };
    let _ = ui.upgrade_in_event_loop(move |ui| {
        ui.set_playing_id(id);
        ui.set_playing_progress(progress);
        ui.set_playing_time(time);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, rate: u32) -> Vec<f32> {
        let n = (seconds * rate as f32) as usize;
        (0..n)
            .map(|i| 0.3 * (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin())
            .collect()
    }

    #[test]
    fn encoded_audio_decodes_to_the_same_length() {
        let pcm = tone(1.234, RATE);
        let ogg = encode(&pcm).unwrap();
        assert_eq!(&ogg[..4], b"OggS");
        // 24 kbit/s: ~3 KB por segundo, mais os cabeçalhos.
        assert!(ogg.len() < 6_000, "{} bytes", ogg.len());
        let back = decode(&ogg).unwrap();
        assert_eq!(back.len(), pcm.len());
        // O som volta parecido (Opus perde detalhe, não volume).
        let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32;
        let ratio = energy(&back) / energy(&pcm);
        assert!((0.5..2.0).contains(&ratio), "energia mudou: {ratio}");
    }

    #[test]
    fn empty_recording_still_makes_a_valid_file() {
        let ogg = encode(&[]).unwrap();
        assert!(decode(&ogg).unwrap().is_empty());
    }

    #[test]
    fn resample_keeps_the_duration() {
        let pcm = tone(1.0, 44_100);
        assert_eq!(resample(&pcm, 44_100, 48_000).len(), 48_000);
        assert_eq!(resample(&pcm, 44_100, 44_100), pcm);
        assert_eq!(resample(&[0.0, 1.0], 1, 2), [0.0, 0.5, 1.0, 1.0]);
    }

    #[test]
    fn durations_are_minutes_and_seconds() {
        assert_eq!(mm_ss(0), "0:00");
        assert_eq!(mm_ss(9_993), "0:10");
        assert_eq!(mm_ss(65_000), "1:05");
        assert_eq!(mm_ss(300_000), "5:00");
    }
}

//! Mídia cifrada ponta a ponta (F2-09).
//!
//! Cada quadro de áudio, câmera e tela é cifrado no app antes de sair e
//! decifrado no app de quem recebe. O LiveKit (e a VPS) só repassa bytes
//! cifrados: sem a chave, não dá para ouvir nem ver nada.
//!
//! Quem cifra é o `FrameCryptor` do próprio WebRTC, ligado pelo SDK do
//! LiveKit (`RoomOptions::encryption`). É o esquema de cifra por quadro do
//! LiveKit, da mesma família do SFrame (RFC 9605): AES-GCM em cada quadro,
//! com o índice da chave no fim do quadro para quem recebe saber qual usar.
//! Não é byte a byte o formato da RFC, mas cumpre o mesmo papel, e é o que
//! o SDK em Rust oferece sem reescrever o transporte.
//!
//! A chave vem do grupo MLS da conversa (F2-08): o segredo exportado com o
//! rótulo [`KEY_LABEL`] na época atual. Todos do grupo chegam ao mesmo
//! segredo, então a chave é uma só para a sala ("shared key"). Quando o
//! grupo muda (entra ou sai gente), a época sobe e vem uma chave nova pelo
//! [`CallHandle::set_key`](crate::call::CallHandle::set_key): quem saiu do
//! grupo não tem a chave nova e para de ouvir a chamada.
//!
//! As chaves ficam num anel de [`RING`] posições, uma por época
//! (`época % RING`). Quem recebe ainda guarda as anteriores, então os
//! quadros que já estavam a caminho com a chave velha continuam abrindo.

use livekit::e2ee::key_provider::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::prelude::*;
use protocol::ConversationId;

/// Rótulo do segredo exportado do grupo MLS para a mídia.
#[allow(dead_code, reason = "quem usa é o MLS da F2-08")]
pub const KEY_LABEL: &str = "mothh sframe";

/// Tamanho do segredo exportado, em bytes.
#[allow(dead_code, reason = "quem usa é o MLS da F2-08")]
pub const KEY_LEN: usize = 32;

/// Quantas chaves (épocas) quem recebe guarda.
const RING: u64 = 16;

/// Sal da derivação da chave AES a partir do segredo.
const SALT: &[u8] = b"mothh sframe";

/// A chave de mídia de uma época do grupo.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaKey {
    /// A época MLS de onde o segredo saiu.
    pub epoch: u64,
    pub secret: Vec<u8>,
}

impl std::fmt::Debug for MediaKey {
    // Não imprime o segredo no log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaKey")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

impl MediaKey {
    /// A posição desta chave no anel.
    fn index(&self) -> i32 {
        (self.epoch % RING) as i32
    }
}

/// Chave fixa de teste, até a F2-08 dar a chave do grupo MLS. Todo mundo
/// chega à mesma chave para a mesma conversa, então a chamada funciona e o
/// LiveKit já só vê quadros cifrados; mas quem conhece o código também
/// chega a ela. Não é segredo nenhum: é só para ligar o caminho.
pub fn test_key(conversation_id: ConversationId) -> MediaKey {
    let mut secret = b"mothh chave de teste F2-09 ".to_vec();
    secret.extend_from_slice(conversation_id.0.as_bytes());
    MediaKey { epoch: 0, secret }
}

/// As opções da sala com a cifra ligada e a primeira chave.
pub fn options(key: &MediaKey) -> E2eeOptions {
    let provider = KeyProvider::with_shared_key(
        KeyProviderOptions {
            // A chave já vem pronta do MLS; não "gira" a chave sozinho quando
            // um quadro não abre, senão um quadro estragado bagunçaria o anel.
            ratchet_window_size: 0,
            ratchet_salt: SALT.to_vec(),
            key_ring_size: RING as i32,
            // O segredo do MLS já é aleatório: HKDF basta (o PBKDF2 padrão
            // é para senha).
            key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
            ..Default::default()
        },
        key.secret.clone(),
    );
    // `with_shared_key` põe a chave na posição 0; a da época vai na dela.
    if key.index() != 0 {
        provider.set_shared_key(key.secret.clone(), key.index());
    }
    E2eeOptions {
        encryption_type: EncryptionType::Gcm,
        key_provider: provider,
    }
}

/// Época nova: guarda a chave no anel e passa a cifrar o que sai com ela.
pub fn set_key(room: &Room, key: &MediaKey) {
    if let Some(provider) = room.e2ee_manager().key_provider() {
        provider.set_shared_key(key.secret.clone(), key.index());
    }
    use_latest_key(room);
}

/// Faz as nossas faixas cifrarem com a chave mais nova. Chamar também
/// depois de publicar uma faixa: a cifra de cada faixa nova começa no
/// índice 0.
pub fn use_latest_key(room: &Room) {
    let manager = room.e2ee_manager();
    let Some(provider) = manager.key_provider() else {
        return;
    };
    let index = provider.get_latest_key_index();
    let me = room.local_participant().identity();
    for ((who, _), cryptor) in manager.frame_cryptors() {
        if who == me {
            cryptor.set_key_index(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chave_de_teste_e_a_mesma_para_todos_da_conversa() {
        let id = ConversationId(uuid::Uuid::new_v4());
        assert_eq!(test_key(id), test_key(id));
        assert_ne!(test_key(id), test_key(ConversationId(uuid::Uuid::new_v4())));
    }

    #[test]
    fn epocas_vao_para_posicoes_do_anel() {
        let key = |epoch| MediaKey {
            epoch,
            secret: vec![],
        };
        assert_eq!(key(0).index(), 0);
        assert_eq!(key(5).index(), 5);
        assert_eq!(key(RING + 1).index(), 1);
    }

    #[test]
    fn segredo_nao_vai_para_o_log() {
        let key = MediaKey {
            epoch: 3,
            secret: b"segredo".to_vec(),
        };
        assert!(!format!("{key:?}").contains("segredo"));
    }

    /// O LiveKit só vê quadros cifrados. Precisa de um LiveKit de verdade:
    ///
    /// ```text
    /// docker run --rm --network host livekit/livekit-server --dev --node-ip 127.0.0.1
    /// MOTHH_LIVEKIT_URL=ws://127.0.0.1:7880 MOTHH_LIVEKIT_KEY=devkey \
    ///   MOTHH_LIVEKIT_SECRET=secret cargo test -p client sframe -- --ignored
    /// ```
    ///
    /// Alguém publica um tom de 440 Hz cifrado. Quem tem a chave ouve o tom;
    /// quem entra sem cifra recebe os bytes como o LiveKit repassou e não
    /// ouve tom nenhum; quem tem a chave errada não abre os quadros. Depois a
    /// chave troca de época e quem tem a nova continua ouvindo.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "precisa de um LiveKit rodando"]
    async fn livekit_so_ve_quadros_cifrados() {
        use std::time::Duration;

        use futures_util::StreamExt;
        use livekit::options::TrackPublishOptions;
        use livekit::webrtc::audio_frame::AudioFrame;
        use livekit::webrtc::audio_source::native::NativeAudioSource;
        use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
        use livekit::webrtc::audio_stream::native::NativeAudioStream;
        use tokio::sync::mpsc::UnboundedReceiver;
        use tokio::time::timeout;

        const RATE: u32 = 48_000;
        const TONE: f32 = 440.0;

        let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("falta {name}"));
        let url = var("MOTHH_LIVEKIT_URL");
        let api_key = var("MOTHH_LIVEKIT_KEY");
        let api_secret = var("MOTHH_LIVEKIT_SECRET");
        let room_name = format!("f2-09-{}", uuid::Uuid::new_v4());
        let token = |who: &str| {
            livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
                .with_identity(who)
                .with_grants(livekit_api::access_token::VideoGrants {
                    room_join: true,
                    room: room_name.clone(),
                    ..Default::default()
                })
                .to_jwt()
                .unwrap()
        };
        let key = |epoch, secret: &[u8]| MediaKey {
            epoch,
            secret: secret.to_vec(),
        };
        let join = |who: &'static str, key: Option<MediaKey>| {
            let url = url.clone();
            let token = token(who);
            async move {
                let mut room_options = RoomOptions::default();
                room_options.encryption = key.as_ref().map(options);
                Room::connect(&url, &token, room_options).await.unwrap()
            }
        };

        let k0 = key(0, b"segredo da epoca zero do grupo..");
        let (fala, _) = join("fala", Some(k0.clone())).await;
        let (certo, mut certo_events) = join("certo", Some(k0.clone())).await;
        let (errado, mut errado_events) =
            join("errado", Some(key(0, b"outro segredo qualquer, errado.."))).await;
        let (sem_cifra, mut sem_cifra_events) = join("sem-cifra", None).await;

        // O tom, em quadros de 10 ms, sem parar até o teste acabar.
        let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 1, 100);
        let track =
            LocalAudioTrack::create_audio_track("tom", RtcAudioSource::Native(source.clone()));
        fala.local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        use_latest_key(&fala);
        let tone = tokio::spawn(async move {
            let mut n = 0u64;
            loop {
                let mut frame = AudioFrame::new(RATE, 1, RATE / 100);
                for sample in frame.data.to_mut() {
                    let t = n as f32 / RATE as f32;
                    *sample = ((t * TONE * std::f32::consts::TAU).sin() * 8000.0) as i16;
                    n += 1;
                }
                source.capture_frame(&frame).await.unwrap();
            }
        });

        // Espera a faixa de `fala` chegar e devolve o áudio dela.
        async fn subscribe(
            events: &mut UnboundedReceiver<RoomEvent>,
        ) -> (EncryptionType, NativeAudioStream) {
            loop {
                if let Some(RoomEvent::TrackSubscribed {
                    track: RemoteTrack::Audio(track),
                    publication,
                    ..
                }) = events.recv().await
                {
                    let stream = NativeAudioStream::new(track.rtc_track(), RATE as i32, 1);
                    return (publication.encryption_type(), stream);
                }
            }
        }
        // Quanto do som é o tom (0 a 1), num segundo de áudio.
        async fn tone_share(stream: &mut NativeAudioStream) -> f32 {
            let mut samples = Vec::new();
            while samples.len() < RATE as usize {
                let frame = stream.next().await.unwrap();
                samples.extend(frame.data.iter().map(|&s| s as f32));
            }
            let energy: f32 = samples.iter().map(|s| s * s).sum();
            if energy < 1.0 {
                return 0.0;
            }
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, s) in samples.iter().enumerate() {
                let angle = std::f32::consts::TAU * TONE * i as f32 / RATE as f32;
                re += s * angle.cos();
                im += s * angle.sin();
            }
            2.0 * (re * re + im * im) / samples.len() as f32 / energy
        }

        let wait = Duration::from_secs(20);
        let (tipo, mut certo_audio) = timeout(wait, subscribe(&mut certo_events)).await.unwrap();
        // O LiveKit sabe que a faixa é cifrada (o anúncio dela diz isso).
        assert_eq!(tipo, EncryptionType::Gcm);
        let (_, mut cru) = timeout(wait, subscribe(&mut sem_cifra_events))
            .await
            .unwrap();

        // Deixa o som chegar e estabilizar.
        tone_share(&mut certo_audio).await;
        let ouvido = tone_share(&mut certo_audio).await;
        assert!(ouvido > 0.5, "quem tem a chave devia ouvir o tom: {ouvido}");
        tone_share(&mut cru).await;
        let vazado = tone_share(&mut cru).await;
        assert!(
            vazado < 0.1,
            "sem a chave não dá para ouvir o tom: {vazado}"
        );

        // Chave errada: os quadros não abrem e não sai tom.
        let (_, mut errado_audio) = timeout(wait, subscribe(&mut errado_events)).await.unwrap();
        tone_share(&mut errado_audio).await;
        let errado_ouviu = tone_share(&mut errado_audio).await;
        assert!(
            errado_ouviu < 0.1,
            "com a chave errada não dá para ouvir: {errado_ouviu}"
        );

        // Época nova (alguém entrou no grupo): todos recebem a chave nova e
        // quem fala passa a cifrar com ela. Quem chega agora só conhece a
        // nova; se ouvir o tom, quem fala trocou mesmo de chave.
        let k1 = key(1, b"segredo da epoca um, depois do commit");
        set_key(&certo, &k1);
        set_key(&fala, &k1);
        let (novo, mut novo_events) = join("novo", Some(k1)).await;
        let (_, mut novo_audio) = timeout(wait, subscribe(&mut novo_events)).await.unwrap();
        tone_share(&mut novo_audio).await;
        let novo_ouviu = tone_share(&mut novo_audio).await;
        assert!(
            novo_ouviu > 0.5,
            "quem só tem a chave nova devia ouvir: {novo_ouviu}"
        );
        let certo_ouviu = tone_share(&mut certo_audio).await;
        assert!(
            certo_ouviu > 0.5,
            "quem já estava devia continuar ouvindo: {certo_ouviu}"
        );

        tone.abort();
        for room in [fala, certo, errado, sem_cifra, novo] {
            room.close().await.unwrap();
        }
    }
}

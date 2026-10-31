// Volume de uma faixa de áudio recebida na chamada (voz ou som da tela de
// alguém). O SDK do LiveKit em Rust não tem isso, mas o WebRTC tem: a fonte
// de uma faixa remota aceita `SetVolume`, que vira o ganho daquela faixa
// antes de ela entrar na mistura que vai para o alto-falante. O cancelamento
// de eco continua vendo o som certo, porque a mistura ainda é do WebRTC.
//
// O Rust entrega o ponteiro do `livekit_ffi::AudioTrack` do webrtc-sys
// (versão presa em =0.3.42 no Cargo.toml). O cabeçalho dele depende de
// arquivos que o webrtc-sys gera no build, então aqui vai só o desenho da
// classe: ela não tem métodos virtuais e guarda, nesta ordem, o runtime
// (`std::shared_ptr`) e a faixa do WebRTC (`scoped_refptr`). Ao subir a
// versão do webrtc-sys, conferir `include/livekit/media_stream_track.h`.

#include <memory>

#include "api/media_stream_interface.h"

namespace {

struct LivekitTrack {
  std::shared_ptr<void> rtc_runtime;
  webrtc::scoped_refptr<webrtc::MediaStreamTrackInterface> track;
};

}  // namespace

// `volume`: 1.0 é o normal, 0.0 é mudo, até 10.0. Devolve se deu certo.
extern "C" bool mothh_set_track_volume(const void* livekit_track,
                                       double volume) {
  if (livekit_track == nullptr) {
    return false;
  }
  auto* wrapper = static_cast<const LivekitTrack*>(livekit_track);
  webrtc::MediaStreamTrackInterface* track = wrapper->track.get();
  if (track == nullptr ||
      track->kind() != webrtc::MediaStreamTrackInterface::kAudioKind) {
    return false;
  }
  auto* audio = static_cast<webrtc::AudioTrackInterface*>(track);
  webrtc::AudioSourceInterface* source = audio->GetSource();
  if (source == nullptr) {
    return false;
  }
  if (volume < 0.0) {
    volume = 0.0;
  } else if (volume > 10.0) {
    volume = 10.0;
  }
  source->SetVolume(volume);
  return true;
}

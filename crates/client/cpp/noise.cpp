// Redução de ruído do microfone na chamada.
//
// O microfone da chamada é gravado pelo próprio WebRTC (o "ADM" da
// plataforma), que entrega cada pedaço de 10 ms a um `AudioTransport`; é ele
// que faz o cancelamento de eco, o ganho e o resto e manda para o Opus. O
// webrtc-sys põe um `AdmProxy` na frente do ADM e guarda esse transporte.
// Aqui colocamos um transporte nosso no meio: ele passa cada pedaço pela
// função do Rust (RNNoise, `noise.rs`) e entrega ao transporte original.
// Tudo o mais (o som que toca no alto-falante) passa direto.
//
// O Rust entrega o `AudioDeviceController` do webrtc-sys, que só guarda o
// `scoped_refptr<AdmProxy>`. O cabeçalho da `AdmProxy` é uma cópia do
// webrtc-sys 0.3.42 (`cpp/livekit/`), preso no Cargo.toml.

#include <cstdint>
#include <cstring>
#include <optional>
#include <vector>

#include "livekit/adm_proxy.h"

namespace {

using Denoise = void (*)(int16_t* samples, size_t frames, size_t channels,
                         uint32_t rate);

// Lê o campo privado `audio_transport_` sem mudar o cabeçalho. É a forma
// que o C++ permite: a instanciação explícita de um template pode nomear um
// membro privado.
struct TransportField {
  using type = webrtc::AudioTransport* livekit_ffi::AdmProxy::*;
  friend type field(TransportField);
};
template <typename Tag, typename Tag::type Member>
struct Expose {
  friend typename Tag::type field(Tag) { return Member; }
};
template struct Expose<TransportField, &livekit_ffi::AdmProxy::audio_transport_>;

struct Controller {
  webrtc::scoped_refptr<livekit_ffi::AdmProxy> adm_proxy;
};

class NoiseTransport : public webrtc::AudioTransport {
 public:
  NoiseTransport(webrtc::AudioTransport* inner, Denoise denoise)
      : inner_(inner), denoise_(denoise) {}

  int32_t RecordedDataIsAvailable(const void* samples, size_t frames,
                                  size_t bytes_per_frame, size_t channels,
                                  uint32_t rate, uint32_t delay_ms,
                                  int32_t drift, uint32_t mic_level,
                                  bool key_pressed,
                                  uint32_t& new_mic_level) override {
    return RecordedDataIsAvailable(samples, frames, bytes_per_frame, channels,
                                   rate, delay_ms, drift, mic_level,
                                   key_pressed, new_mic_level, std::nullopt);
  }

  int32_t RecordedDataIsAvailable(
      const void* samples, size_t frames, size_t bytes_per_frame,
      size_t channels, uint32_t rate, uint32_t delay_ms, int32_t drift,
      uint32_t mic_level, bool key_pressed, uint32_t& new_mic_level,
      std::optional<int64_t> capture_time_ns) override {
    const void* data = samples;
    // Só PCM de 16 bits (o que todo ADM entrega).
    if (samples != nullptr && channels > 0 &&
        bytes_per_frame == channels * sizeof(int16_t)) {
      buffer_.resize(frames * channels);
      std::memcpy(buffer_.data(), samples, frames * bytes_per_frame);
      denoise_(buffer_.data(), frames, channels, rate);
      data = buffer_.data();
    }
    return inner_->RecordedDataIsAvailable(
        data, frames, bytes_per_frame, channels, rate, delay_ms, drift,
        mic_level, key_pressed, new_mic_level, capture_time_ns);
  }

  int32_t NeedMorePlayData(size_t frames, size_t bytes_per_frame,
                           size_t channels, uint32_t rate, void* samples,
                           size_t& frames_out, int64_t* elapsed_time_ms,
                           int64_t* ntp_time_ms) override {
    return inner_->NeedMorePlayData(frames, bytes_per_frame, channels, rate,
                                    samples, frames_out, elapsed_time_ms,
                                    ntp_time_ms);
  }

  void PullRenderData(int bits_per_sample, int rate, size_t channels,
                      size_t frames, void* data, int64_t* elapsed_time_ms,
                      int64_t* ntp_time_ms) override {
    inner_->PullRenderData(bits_per_sample, rate, channels, frames, data,
                           elapsed_time_ms, ntp_time_ms);
  }

 private:
  webrtc::AudioTransport* inner_;
  Denoise denoise_;
  std::vector<int16_t> buffer_;
};

// Os nossos já colocados (nunca são apagados: o ADM pode usar até o fim).
std::vector<NoiseTransport*>& installed() {
  static std::vector<NoiseTransport*> list;
  return list;
}

}  // namespace

// 0: colocou; 1: já estava; -1: o WebRTC ainda não tem transporte.
extern "C" int mothh_noise_install(const void* controller, Denoise denoise) {
  if (controller == nullptr || denoise == nullptr) {
    return -1;
  }
  livekit_ffi::AdmProxy* proxy =
      static_cast<const Controller*>(controller)->adm_proxy.get();
  if (proxy == nullptr) {
    return -1;
  }
  webrtc::AudioTransport* current = proxy->*field(TransportField{});
  if (current == nullptr) {
    return -1;
  }
  for (NoiseTransport* ours : installed()) {
    if (ours == current) {
      return 1;
    }
  }
  auto* ours = new NoiseTransport(current, denoise);
  installed().push_back(ours);
  proxy->RegisterAudioCallback(ours);
  return 0;
}

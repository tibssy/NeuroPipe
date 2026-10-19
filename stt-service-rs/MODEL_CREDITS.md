# Model Credits

Models used by the NeuroPipe STT service (`neuro-stt-service`).

## Smart Turn v3.2 (turn-end detection)

The smart-turn model is from **[pipecat-ai/smart-turn](https://github.com/pipecat-ai/smart-turn)** and is licensed under **BSD-2-Clause**.

- Model weights: https://huggingface.co/pipecat-ai/smart-turn-v3
- Code: https://github.com/pipecat-ai/smart-turn

BSD-2-Clause permits commercial use and modification with attribution; it does not restrict use.

## Silero VAD

The VAD model is from **[snakers4/silero-vad](https://github.com/snakers4/silero-vad)** (exported to ONNX by [onnx-community](https://huggingface.co/onnx-community/silero-vad)) and is licensed under **MIT**.

## Parakeet TDT 0.6B V3 (ASR)

The ASR model is **[NVIDIA Parakeet TDT 0.6B V3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)**, converted to ONNX by [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx), and is licensed under **CC-BY-4.0**.

- Model card: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3
- License: https://creativecommons.org/licenses/by/4.0

## Parakeet Redux (optional ASR)

The optional alternate ASR model is **[Moondream Parakeet Redux](https://huggingface.co/moondream/parakeet-redux)**, a 1.58-bit ternary quantization of NVIDIA Parakeet TDT 0.6B V3, exported to ONNX by [eschmidbauer/parakeet-redux-onnx](https://huggingface.co/eschmidbauer/parakeet-redux-onnx). It is licensed under **CC-BY-4.0** (inherited from the base model).

- Model card: https://huggingface.co/moondream/parakeet-redux
- ONNX export: https://huggingface.co/eschmidbauer/parakeet-redux-onnx
- License: https://creativecommons.org/licenses/by/4.0/

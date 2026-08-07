#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
NEUROPIPE_CONFIG_DIR="$HOME/.config/neuropipe"
NEUROPIPE_CONFIG_FILE="$NEUROPIPE_CONFIG_DIR/config.toml"

TTS_DIR="$ROOT_DIR/tts-service-rs"
STT_DIR="$ROOT_DIR/stt-service-rs"
ASSISTANT_DIR="$ROOT_DIR/assistant-service-rs"
CLI_DIR="$ROOT_DIR/cli"
RELEASE_API_URL="https://api.github.com/repos/tibssy/NeuroPipe/releases/latest"

declare -a SERVICE_UNITS=()
PKG_MANAGER=""
ASSISTANT_DEFAULT_MODEL_OVERRIDE=""

clear_screen() {
  if command -v clear >/dev/null 2>&1; then
    clear
  else
    printf "\033[2J\033[H"
  fi
}

print_header() {
  printf "\n\e[34m====================================================\n"
  printf "  Welcome to the NeuroPipe Installer\n"
  printf "====================================================\e[0m\n"
}

print_description() {
  printf "\e[36mNeuroPipe is a local-first speech pipeline for Linux that\n"
  printf "connects STT and TTS services through fast IPC sockets.\e[0m\n"
}

require_command() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    printf "\e[31mError: '%s' is required but not installed.\e[0m\n" "$cmd"
    return 1
  fi
}

detect_package_manager() {
  if [[ -n "$PKG_MANAGER" ]]; then
    return 0
  fi

  if command -v pacman >/dev/null 2>&1; then
    PKG_MANAGER="pacman"
  elif command -v apt-get >/dev/null 2>&1; then
    PKG_MANAGER="apt"
  elif command -v dnf >/dev/null 2>&1; then
    PKG_MANAGER="dnf"
  elif command -v zypper >/dev/null 2>&1; then
    PKG_MANAGER="zypper"
  elif command -v apk >/dev/null 2>&1; then
    PKG_MANAGER="apk"
  else
    PKG_MANAGER="unknown"
  fi
}

print_dependency_install_hint() {
  local profile="$1"
  detect_package_manager

  printf "\n\e[36mSuggested install command for %s:\e[0m\n" "$profile"

  case "$PKG_MANAGER" in
    pacman)
      if [[ "$profile" == "runtime" ]]; then
        printf "  sudo pacman -S --needed systemd\n"
      else
        printf "  sudo pacman -S --needed base-devel\n"
      fi
      ;;
    apt)
      if [[ "$profile" == "runtime" ]]; then
        printf "  sudo apt-get install -y systemd\n"
      else
        printf "  sudo apt-get install -y build-essential\n"
      fi
      ;;
    dnf)
      if [[ "$profile" == "runtime" ]]; then
        printf "  sudo dnf install -y systemd\n"
      else
        printf "  sudo dnf install -y gcc gcc-c++ make\n"
      fi
      ;;
    zypper)
      if [[ "$profile" == "runtime" ]]; then
        printf "  sudo zypper install -y systemd\n"
      else
        printf "  sudo zypper install -y gcc gcc-c++ make\n"
      fi
      ;;
    apk)
      if [[ "$profile" == "runtime" ]]; then
        printf "  sudo apk add systemd\n"
      else
        printf "  sudo apk add build-base\n"
      fi
      ;;
    *)
      printf "  Install required packages manually for your distro.\n"
      ;;
  esac

  if [[ "$profile" == "build" ]]; then
    printf "  # Rust toolchain (cargo)\n"
    printf "  curl https://sh.rustup.rs -sSf | sh\n"
  fi
}

selection_has_assistant() {
  local selection="$1"
  [[ "$selection" == "3" || "$selection" == "4" ]]
}

check_runtime_dependencies() {
  local selection="$1"
  local -a missing=()

  command -v systemctl >/dev/null 2>&1 || missing+=("systemctl")

  if selection_has_assistant "$selection"; then
    command -v ollama >/dev/null 2>&1 || missing+=("ollama")
  fi

  if [[ ${#missing[@]} -gt 0 ]]; then
    printf "\e[31mError: Missing required dependencies:\e[0m\n"
    printf "  - %s\n" "${missing[@]}"
    print_dependency_install_hint "runtime"
    if selection_has_assistant "$selection"; then
      printf "  Install Ollama: https://ollama.com/download\n"
    fi
    return 1
  fi
}

check_build_dependencies() {
  local selection="$1"
  local -a missing=()

  command -v gcc >/dev/null 2>&1 || missing+=("gcc")
  command -v g++ >/dev/null 2>&1 || missing+=("g++")
  command -v make >/dev/null 2>&1 || missing+=("make")
  command -v cargo >/dev/null 2>&1 || missing+=("cargo")

  if [[ ${#missing[@]} -gt 0 ]]; then
    printf "\e[31mError: Missing required build dependencies:\e[0m\n"
    printf "  - %s\n" "${missing[@]}"
    print_dependency_install_hint "build"
    return 1
  fi

  if selection_has_assistant "$selection"; then
    require_command ollama
  fi
}

verify_build_prerequisites() {
  local selection="$1"
  check_runtime_dependencies "$selection"
  check_build_dependencies "$selection"
}

verify_prebuilt_prerequisites() {
  local selection="$1"
  require_command curl
  check_runtime_dependencies "$selection"
}

prepare_install_dirs() {
  mkdir -p "$LOCAL_BIN_DIR"
  mkdir -p "$SYSTEMD_USER_DIR"
}

build_component() {
  local component_name="$1"
  local component_dir="$2"
  local binary_name="$3"
  local unit_name="$4"

  local build_script="$component_dir/build.sh"
  local built_binary="$component_dir/dist/$binary_name"
  local unit_source="$component_dir/src/service/$unit_name"

  printf "\n\e[34mBuilding %s...\e[0m\n" "$component_name"

  if [[ ! -f "$build_script" ]]; then
    printf "\e[31mError: Missing build script at %s\e[0m\n" "$build_script"
    return 1
  fi

  if [[ ! -f "$unit_source" ]]; then
    printf "\e[31mError: Missing service file at %s\e[0m\n" "$unit_source"
    return 1
  fi

  chmod +x "$build_script"

  if ! (cd "$component_dir" && ./build.sh); then
    printf "\e[31mError: Build failed for %s.\e[0m\n" "$component_name"
    return 1
  fi

  if [[ ! -f "$built_binary" ]]; then
    printf "\e[31mError: Built binary not found at %s\e[0m\n" "$built_binary"
    return 1
  fi

  printf "\e[32mBuild verification passed: %s\e[0m\n" "$built_binary"
}

install_component_files() {
  local component_dir="$1"
  local binary_name="$2"
  local unit_name="$3"

  local built_binary="$component_dir/dist/$binary_name"
  local unit_source="$component_dir/src/service/$unit_name"

  if [[ ! -f "$built_binary" ]]; then
    printf "\e[31mError: Cannot install, missing binary at %s\e[0m\n" "$built_binary"
    return 1
  fi

  if [[ ! -f "$unit_source" ]]; then
    printf "\e[31mError: Cannot install, missing service file at %s\e[0m\n" "$unit_source"
    return 1
  fi

  install -Dm755 "$built_binary" "$LOCAL_BIN_DIR/$binary_name"
  install -Dm644 "$unit_source" "$SYSTEMD_USER_DIR/$unit_name"

  SERVICE_UNITS+=("$unit_name")

  printf "\e[32mInstalled binary: %s\e[0m\n" "$LOCAL_BIN_DIR/$binary_name"
  printf "\e[32mInstalled service: %s\e[0m\n" "$SYSTEMD_USER_DIR/$unit_name"
}

install_cli_binary() {
  local dist_binary="$CLI_DIR/dist/neuro-ipc"
  if [[ -f "$dist_binary" ]]; then
    install -Dm755 "$dist_binary" "$LOCAL_BIN_DIR/neuro-ipc"
    printf "\e[32mInstalled binary: %s/neuro-ipc\e[0m\n" "$LOCAL_BIN_DIR"
    return 0
  fi
  if command -v cargo &>/dev/null; then
    printf "\n\e[34mBuilding neuro-ipc from source...\e[0m\n"
    (cd "$CLI_DIR" && cargo build --release) || {
      printf "\e[31mRust build failed. Install Rust via https://rustup.rs\e[0m\n"
      return 1
    }
    install -Dm755 "$CLI_DIR/target/release/neuro-ipc" "$LOCAL_BIN_DIR/neuro-ipc"
  else
    printf "\e[31mError: no prebuilt binary found and Rust is not installed.\e[0m\n"
    printf "\e[33mEither install Rust (https://rustup.rs) or use a release with prebuilt binaries.\e[0m\n"
    return 1
  fi
  printf "\e[32mInstalled binary: %s/neuro-ipc\e[0m\n" "$LOCAL_BIN_DIR"
}

build_cli_binary() {
  local target_binary="$CLI_DIR/target/release/neuro-ipc"

  if ! command -v cargo &>/dev/null; then
    printf "\e[31mError: cargo is required to build neuro-ipc from source.\e[0m\n"
    printf "\e[33mInstall Rust via https://rustup.rs and run installer again.\e[0m\n"
    return 1
  fi

  printf "\n\e[34mBuilding neuro-ipc from source...\e[0m\n"
  (cd "$CLI_DIR" && cargo build --release) || {
    printf "\e[31mRust build failed. Install Rust via https://rustup.rs\e[0m\n"
    return 1
  }

  if [[ ! -f "$target_binary" ]]; then
    printf "\e[31mError: Built CLI binary not found at %s\e[0m\n" "$target_binary"
    return 1
  fi

  printf "\e[32mBuild verification passed: %s\e[0m\n" "$target_binary"
}

install_cli_binary_from_source() {
  local target_binary="$CLI_DIR/target/release/neuro-ipc"

  if [[ ! -f "$target_binary" ]]; then
    printf "\e[31mError: Cannot install, missing CLI binary at %s\e[0m\n" "$target_binary"
    return 1
  fi

  install -Dm755 "$target_binary" "$LOCAL_BIN_DIR/neuro-ipc"
  printf "\e[32mInstalled binary: %s/neuro-ipc\e[0m\n" "$LOCAL_BIN_DIR"
}

install_default_config() {
  local config_template="$ROOT_DIR/config.example.toml"

  if [[ ! -f "$config_template" ]]; then
    printf "\e[33mWarning: missing config template at %s, skipping config install.\e[0m\n" "$config_template"
    return 0
  fi

  mkdir -p "$NEUROPIPE_CONFIG_DIR"

  if [[ -f "$NEUROPIPE_CONFIG_FILE" ]]; then
    printf "\e[32mKeeping existing config: %s\e[0m\n" "$NEUROPIPE_CONFIG_FILE"
    return 0
  fi

  install -Dm644 "$config_template" "$NEUROPIPE_CONFIG_FILE"

  if [[ -n "$ASSISTANT_DEFAULT_MODEL_OVERRIDE" ]]; then
    local tmp_file
    tmp_file="$(mktemp)"
    local replaced=false
    local line
    while IFS= read -r line || [[ -n "$line" ]]; do
      if [[ "$replaced" == false && "$line" =~ ^default_model[[:space:]]*= ]]; then
        printf 'default_model = "%s"\n' "$ASSISTANT_DEFAULT_MODEL_OVERRIDE" >> "$tmp_file"
        replaced=true
      else
        printf '%s\n' "$line" >> "$tmp_file"
      fi
    done < "$NEUROPIPE_CONFIG_FILE"
    mv "$tmp_file" "$NEUROPIPE_CONFIG_FILE"
  fi

  printf "\e[32mInstalled default config: %s\e[0m\n" "$NEUROPIPE_CONFIG_FILE"
}

select_assistant_default_model() {
  local default_model="llama3.2:1b"

  ASSISTANT_DEFAULT_MODEL_OVERRIDE=""

  local ollama_output
  if ! ollama_output="$(ollama list 2>/dev/null)"; then
    printf "\e[31mError: failed to run 'ollama list'. Make sure Ollama is installed and running.\e[0m\n"
    return 1
  fi

  local -a models=()
  local line
  local name
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    [[ "$line" =~ ^NAME[[:space:]]+ID[[:space:]]+SIZE[[:space:]]+MODIFIED ]] && continue
    name="${line%%[[:space:]]*}"
    [[ -z "$name" ]] && continue
    models+=("$name")
  done <<< "$ollama_output"

  if [[ ${#models[@]} -eq 0 ]]; then
    printf "\e[31mError: no Ollama models found. Pull one first (for example: ollama pull %s).\e[0m\n" "$default_model"
    return 1
  fi

  printf "\n\e[36mSelect default assistant model (from ollama list):\e[0m\n"
  local i
  for i in "${!models[@]}"; do
    printf "  %d) %s\n" "$((i + 1))" "${models[$i]}"
  done

  local choice
  while true; do
    printf "\e[36mEnter model number [1-%d]: \e[0m" "${#models[@]}"
    read -r choice
    if [[ "$choice" =~ ^[0-9]+$ ]] && (( choice >= 1 && choice <= ${#models[@]} )); then
      ASSISTANT_DEFAULT_MODEL_OVERRIDE="${models[$((choice - 1))]}"
      printf "\e[32mSelected default assistant model: %s\e[0m\n" "$ASSISTANT_DEFAULT_MODEL_OVERRIDE"
      return 0
    fi
    printf "\e[31mInvalid selection. Please enter a number between 1 and %d.\e[0m\n" "${#models[@]}"
  done
}

install_tool_plugins() {
  local tools_src="$ASSISTANT_DIR/tools"
  local tools_dst="$HOME/.local/share/neuropipe/tools"

  if [[ ! -d "$tools_src" ]]; then
    printf "\e[33mNo tool plugins found at %s, skipping.\e[0m\n" "$tools_src"
    return 0
  fi

  mkdir -p "$tools_dst"
  cp -r "$tools_src"/* "$tools_dst/"

  # Install Python dependencies for tools
  if command -v pip3 &>/dev/null; then
    pip3 install --break-system-packages --user ddgs 2>/dev/null || \
      pip3 install --user ddgs 2>/dev/null || \
      printf "\e[33mWarning: could not install ddgs. web_search tool may not work.\e[0m\n"
  fi

  printf "\e[32mInstalled tool plugins: "
  local first=true
  for d in "$tools_src"/*/; do
    if [[ -d "$d" ]]; then
      if [[ "$first" == true ]]; then
        printf "%s" "$(basename "$d")"
        first=false
      else
        printf ", %s" "$(basename "$d")"
      fi
    fi
  done
  printf "\e[0m\n"
}

pull_assistant_embedding_model() {
  local model_name="all-minilm"

  require_command ollama

  printf "\n\e[34mPulling Ollama embedding model (%s)...\e[0m\n" "$model_name"
  ollama pull "$model_name"
  printf "\e[32mPulled Ollama model: %s\e[0m\n" "$model_name"
}

handle_pocket_tts_voices() {
  local voices_dir="$HOME/.local/share/neuropipe/models/pocket-tts/voices"

  printf "\n\e[36mPocket-tts voice embeddings are from Kyutai and licensed under CC-BY-4.0.\e[0m\n"
  printf "\e[36mSee https://github.com/tibssy/NeuroPipe/blob/main/tts-service-rs/VOICE_CREDITS.md\e[0m\n"

  if [[ -d "$voices_dir" && "$(ls -A "$voices_dir" 2>/dev/null)" ]]; then
    printf "\n\e[32mVoices already present in %s\e[0m\n" "$voices_dir"
    return 0
  fi

  printf "\n\e[36mPre-download pocket-tts voice embeddings? [y/N]: \e[0m"
  local answer
  read -r answer
  if [[ ! "$answer" =~ ^[yY] ]]; then
    printf "\n\e[33mCustom voices: neuro-ipc tts set-state --voice /path/to/voice.safetensors\e[0m\n"
    return 0
  fi

  local url="https://github.com/tibssy/NeuroPipe/releases/download/v0.4.0/pocket-tts-voices.zip"
  printf "\n\e[34mDownloading pocket-tts voices...\e[0m\n"

  mkdir -p "$voices_dir"
  if curl -fSL "$url" -o /tmp/pocket-tts-voices.zip; then
    unzip -o /tmp/pocket-tts-voices.zip -d "$voices_dir"
    rm /tmp/pocket-tts-voices.zip
    printf "\e[32mVoices downloaded to %s\e[0m\n" "$voices_dir"
  else
    printf "\e[31mDownload failed. Check your internet connection or use custom safetensors.\e[0m\n"
    return 1
  fi
}

handle_kokoro_models() {
  local models_dir="$HOME/.local/share/neuropipe/models/kokoro"

  if [[ -f "$models_dir/kokoro-v1.0.fp16.onnx" && -f "$models_dir/kokoro-v1.0.onnx" && -f "$models_dir/voices-v1.0.bin" ]]; then
    printf "\n\e[32mKokoro models already present in %s\e[0m\n" "$models_dir"
    return 0
  fi

  printf "\n\e[36mKokoro TTS model is from thewh1teagle/kokoro-onnx (Apache 2.0 / MIT).\e[0m\n"
  printf "\e[36mSee https://github.com/thewh1teagle/kokoro-onnx\e[0m\n"
  printf "\n\e[36mPre-download Kokoro TTS model files (~480MB total)? [y/N]: \e[0m"
  local answer
  read -r answer
  if [[ ! "$answer" =~ ^[yY] ]]; then
    printf "\n\e[33mKokoro models were not downloaded; download them before using the Kokoro engine.\e[0m\n"
    return 0
  fi

  local base_url="https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0"
  mkdir -p "$models_dir"

  printf "\n\e[34mDownloading kokoro-v1.0.fp16.onnx (~170MB)...\e[0m\n"
  if curl -fSL "$base_url/kokoro-v1.0.fp16.onnx" -o "$models_dir/kokoro-v1.0.fp16.onnx"; then
    printf "\e[32mDone.\e[0m\n"
  else
    printf "\e[31mDownload failed.\e[0m\n"
    return 1
  fi

  printf "\n\e[34mDownloading kokoro-v1.0.onnx (~310MB)...\e[0m\n"
  if curl -fSL "$base_url/kokoro-v1.0.onnx" -o "$models_dir/kokoro-v1.0.onnx"; then
    printf "\e[32mDone.\e[0m\n"
  else
    printf "\e[31mDownload failed.\e[0m\n"
    return 1
  fi

  printf "\n\e[34mDownloading voices-v1.0.bin...\e[0m\n"
  if curl -fSL "$base_url/voices-v1.0.bin" -o "$models_dir/voices-v1.0.bin"; then
    printf "\e[32mDone.\e[0m\n"
  else
    printf "\e[31mDownload failed.\e[0m\n"
    return 1
  fi

  printf "\e[32mKokoro models downloaded to %s\e[0m\n" "$models_dir"
}

download_stt_models() {
  local model_dir="$HOME/.local/share/neuropipe/stt/parakeet-v3"
  local vad_dir="$HOME/.local/share/neuropipe/stt"
  local base_url="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"
  local vad_url="https://huggingface.co/onnx-community/silero-vad/resolve/main/onnx/model.onnx"
  local smart_turn_url="https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx"

  if [[ -f "$model_dir/config.json" &&
        -f "$model_dir/vocab.txt" &&
        -f "$model_dir/encoder-model.int8.onnx" &&
        -f "$model_dir/decoder_joint-model.int8.onnx" &&
        -f "$vad_dir/silero_vad.onnx" &&
        -f "$vad_dir/smart_turn_v3.2_cpu.onnx" ]]; then
    printf "\n\e[32mSTT models already present in %s\e[0m\n" "$model_dir"
    return 0
  fi

  printf "\n\e[34mDownloading Parakeet TDT v3 int8 STT model...\e[0m\n"
  mkdir -p "$model_dir" "$vad_dir"
  curl -fL --progress-bar "$base_url/config.json" -o "$model_dir/config.json"
  curl -fL --progress-bar "$base_url/vocab.txt" -o "$model_dir/vocab.txt"
  curl -fL --progress-bar "$base_url/encoder-model.int8.onnx" -o "$model_dir/encoder-model.int8.onnx"
  curl -fL --progress-bar "$base_url/decoder_joint-model.int8.onnx" -o "$model_dir/decoder_joint-model.int8.onnx"
  printf "\n\e[34mDownloading Silero VAD model...\e[0m\n"
  curl -fL --progress-bar "$vad_url" -o "$vad_dir/silero_vad.onnx"
  printf "\n\e[34mDownloading Smart Turn v3.2 model (BSD-2-Clause, pipecat-ai)...\e[0m\n"
  curl -fL --progress-bar "$smart_turn_url" -o "$vad_dir/smart_turn_v3.2_cpu.onnx"
  printf "\e[32mSTT models downloaded to %s\e[0m\n" "$model_dir"
}

prompt_install_approval() {
  local artifacts_label="$1"
  printf "\n\e[36m%s are ready.\e[0m\n" "$artifacts_label"
  printf "\e[36mProceed with copying files and enabling/starting services? [y/N]: \e[0m"

  local answer
  read -r answer
  case "$answer" in
    y|Y|yes|YES)
      return 0
      ;;
    *)
      printf "\n\e[33mInstallation cancelled by user. Exiting without file copy or service changes.\e[0m\n"
      exit 0
      ;;
  esac
}

detect_linux_arch() {
  local raw_arch
  raw_arch="$(uname -m)"

  case "$raw_arch" in
    x86_64)
      printf "x86_64"
      ;;
    aarch64|arm64)
      printf "arm64"
      ;;
    *)
      printf "\e[31mError: Unsupported architecture: %s\e[0m\n" "$raw_arch" >&2
      return 1
      ;;
  esac
}

download_prebuilt_binary() {
  local component_name="$1"
  local component_dir="$2"
  local binary_name="$3"
  local release_arch="$4"

  local release_asset_name="${binary_name}-linux-${release_arch}"
  local output_binary="$component_dir/dist/$binary_name"
  local release_json
  local download_url

  printf "\n\e[34mFetching prebuilt %s (%s)...\e[0m\n" "$component_name" "$release_arch"

  release_json="$(curl -fsSL "$RELEASE_API_URL")"
  download_url="$(printf "%s" "$release_json" | grep -Eo "\"browser_download_url\": \"[^\"]*${release_asset_name}\"" | cut -d '"' -f 4)"

  if [[ -z "$download_url" ]]; then
    printf "\e[31mError: Could not find release asset '%s' in latest release.\e[0m\n" "$release_asset_name"
    return 1
  fi

  mkdir -p "$component_dir/dist"
  curl -fL "$download_url" -o "$output_binary"
  chmod +x "$output_binary"

  if [[ ! -f "$output_binary" ]]; then
    printf "\e[31mError: Download verification failed at %s\e[0m\n" "$output_binary"
    return 1
  fi

  printf "\e[32mDownload verification passed: %s\e[0m\n" "$output_binary"
}

enable_and_start_services() {
  if [[ ${#SERVICE_UNITS[@]} -eq 0 ]]; then
    return 0
  fi

  printf "\n\e[34mReloading systemd user units...\e[0m\n"
  systemctl --user daemon-reload

  for unit_name in "${SERVICE_UNITS[@]}"; do
    printf "\n\e[34mEnabling and starting %s...\e[0m\n" "$unit_name"
    systemctl --user enable --now "$unit_name"

    if systemctl --user is-active --quiet "$unit_name"; then
      printf "\e[32m%s is active.\e[0m\n" "$unit_name"
    else
      printf "\e[31mWarning: %s is not active. Check: systemctl --user status %s\e[0m\n" "$unit_name" "$unit_name"
      return 1
    fi
  done
}

print_usage_examples() {
  local selection="$1"

  printf "\n\e[34mQuick usage examples\e[0m\n"
  printf "\n\e[36mConfig (bash):\e[0m\n"
  printf "  %s/neuro-ipc config path\n" "$LOCAL_BIN_DIR"

  if [[ "$selection" == "1" || "$selection" == "4" ]]; then
    printf "\n\e[36mTTS (bash):\e[0m\n"
    printf "  %s/neuro-ipc tts speak \"Hello from NeuroPipe\"\n" "$LOCAL_BIN_DIR"
    printf "  %s/neuro-ipc tts stop\n" "$LOCAL_BIN_DIR"
    printf "  systemctl --user status neuropipe-tts.service\n"
  fi

  if [[ "$selection" == "2" || "$selection" == "4" ]]; then
    printf "\n\e[36mSTT (bash):\e[0m\n"
    printf "  text=\$(%s/neuro-ipc stt trigger) && printf 'Heard: %%s\\n' \"\$text\"\n" "$LOCAL_BIN_DIR"
    printf "  systemctl --user status neuropipe-stt.service\n"

    printf "\n\e[36mHyprland example binding:\e[0m\n"
    printf "  bind = SUPER, V, exec, bash -lc 'text=\$(%s/neuro-ipc stt trigger); [ -n \"\$text\" ] && wtype -d 5 \"\$text\"'\n" "$LOCAL_BIN_DIR"

    printf "\n\e[36mNiri example binding:\e[0m\n"
    printf "  Mod+V { spawn \"bash\" \"-lc\" \"text=\$(%s/neuro-ipc stt trigger); [ -n \\\"\$text\\\" ] && wtype -d 5 \\\"\$text\\\"\"; }\n" "$LOCAL_BIN_DIR"
  fi

  if [[ "$selection" == "3" || "$selection" == "4" ]]; then
    printf "\n\e[36mAssistant (bash):\e[0m\n"
    printf "  %s/neuro-ipc assistant mode2 --model gemma4:cloud\n" "$LOCAL_BIN_DIR"
    printf "  %s/neuro-ipc assistant interrupt\n" "$LOCAL_BIN_DIR"
    printf "  systemctl --user status neuropipe-assistant.service\n"

    printf "\n\e[36mHyprland example binding:\e[0m\n"
    printf "  bind = SUPER, Period, exec, %s/neuro-ipc assistant mode2 --model gemma4:cloud\n" "$LOCAL_BIN_DIR"
    printf "  bind = SUPER, comma, exec, %s/neuro-ipc assistant interrupt\n" "$LOCAL_BIN_DIR"
  fi
}

run_build_flow() {
  local selection="$1"

  verify_build_prerequisites "$selection"
  SERVICE_UNITS=()

  if [[ "$selection" == "3" || "$selection" == "4" ]]; then
    select_assistant_default_model
  fi

  case "$selection" in
    1)
      build_component "TTS Service" "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      ;;
    2)
      build_component "STT Service" "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    3)
      build_component "Assistant Service" "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
    4)
      build_component "TTS Service" "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      build_component "STT Service" "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      build_component "Assistant Service" "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
    *)
      printf "\e[31mUnknown build selection: %s\e[0m\n" "$selection"
      return 1
      ;;
  esac

  build_cli_binary

  prompt_install_approval "Build artifacts"
  prepare_install_dirs

  case "$selection" in
    1)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      ;;
    2)
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    3)
      install_component_files "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
    4)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      install_component_files "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
  esac
  install_cli_binary_from_source

  if [[ "$selection" == "2" || "$selection" == "4" ]]; then
    download_stt_models
  fi

  if [[ "$selection" == "3" || "$selection" == "4" ]]; then
    install_tool_plugins
    pull_assistant_embedding_model
  fi

  install_default_config

  enable_and_start_services

  if [[ "$selection" == "1" || "$selection" == "4" ]]; then
    handle_pocket_tts_voices
    handle_kokoro_models
  fi

  printf "\n\e[32mBuild and installation finished successfully.\e[0m\n"
  print_usage_examples "$selection"
}

run_prebuilt_flow() {
  local selection="$1"
  local release_arch

  verify_prebuilt_prerequisites "$selection"
  release_arch="$(detect_linux_arch)"
  SERVICE_UNITS=()

  if [[ "$selection" == "3" || "$selection" == "4" ]]; then
    select_assistant_default_model
  fi

  case "$selection" in
    1)
      download_prebuilt_binary "TTS Service" "$TTS_DIR" "neuro-tts-service" "$release_arch"
      download_prebuilt_binary "CLI" "$CLI_DIR" "neuro-ipc" "$release_arch"
      ;;
    2)
      download_prebuilt_binary "STT Service" "$STT_DIR" "neuro-stt-service" "$release_arch"
      download_prebuilt_binary "CLI" "$CLI_DIR" "neuro-ipc" "$release_arch"
      ;;
    3)
      download_prebuilt_binary "Assistant Service" "$ASSISTANT_DIR" "neuro-assistant-service" "$release_arch"
      download_prebuilt_binary "CLI" "$CLI_DIR" "neuro-ipc" "$release_arch"
      ;;
    4)
      download_prebuilt_binary "TTS Service" "$TTS_DIR" "neuro-tts-service" "$release_arch"
      download_prebuilt_binary "STT Service" "$STT_DIR" "neuro-stt-service" "$release_arch"
      download_prebuilt_binary "Assistant Service" "$ASSISTANT_DIR" "neuro-assistant-service" "$release_arch"
      download_prebuilt_binary "CLI" "$CLI_DIR" "neuro-ipc" "$release_arch"
      ;;
    *)
      printf "\e[31mUnknown prebuilt selection: %s\e[0m\n" "$selection"
      return 1
      ;;
  esac

  prompt_install_approval "Downloaded binaries"
  prepare_install_dirs

  case "$selection" in
    1)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      ;;
    2)
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    3)
      install_component_files "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
    4)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      install_component_files "$ASSISTANT_DIR" "neuro-assistant-service" "neuropipe-assistant.service"
      ;;
  esac
  install_cli_binary

  if [[ "$selection" == "2" || "$selection" == "4" ]]; then
    download_stt_models
  fi

  if [[ "$selection" == "3" || "$selection" == "4" ]]; then
    install_tool_plugins
    pull_assistant_embedding_model
  fi

  install_default_config

  enable_and_start_services

  if [[ "$selection" == "1" || "$selection" == "4" ]]; then
    handle_pocket_tts_voices
    handle_kokoro_models
  fi

  printf "\n\e[32mPrebuilt installation finished successfully.\e[0m\n"
  print_usage_examples "$selection"
}

select_build_targets() {
  clear_screen
  print_header
  print_description
  printf "\n\e[36mWhat would you like to build?\e[0m\n"

  select _choice in "Build TTS" "Build STT" "Build Assistant" "Build all services" "Back"; do
    case "${REPLY}" in
      1)
        clear_screen
        print_header
        print_description
        if run_build_flow 1; then
          return 0
        fi
        printf "\n\e[33mBuild failed. Returning to build menu.\e[0m\n"
        ;;
      2)
        clear_screen
        print_header
        print_description
        if run_build_flow 2; then
          return 0
        fi
        printf "\n\e[33mBuild failed. Returning to build menu.\e[0m\n"
        ;;
      3)
        clear_screen
        print_header
        print_description
        if run_build_flow 3; then
          return 0
        fi
        printf "\n\e[33mBuild failed. Returning to build menu.\e[0m\n"
        ;;
      4)
        clear_screen
        print_header
        print_description
        if run_build_flow 4; then
          return 0
        fi
        printf "\n\e[33mBuild failed. Returning to build menu.\e[0m\n"
        ;;
      5)
        clear_screen
        return 1
        ;;
      *)
        printf "\e[31mInvalid option. Please choose 1, 2, 3, 4, or 5.\e[0m\n"
        ;;
    esac
  done
}

select_prebuilt_targets() {
  clear_screen
  print_header
  print_description
  printf "\n\e[36mWhich prebuilt binaries would you like to install?\e[0m\n"

  select _choice in "TTS only" "STT only" "Assistant only" "All services" "Back"; do
    case "${REPLY}" in
      1)
        clear_screen
        print_header
        print_description
        if run_prebuilt_flow 1; then
          return 0
        fi
        printf "\n\e[33mPrebuilt install failed. Returning to prebuilt menu.\e[0m\n"
        ;;
      2)
        clear_screen
        print_header
        print_description
        if run_prebuilt_flow 2; then
          return 0
        fi
        printf "\n\e[33mPrebuilt install failed. Returning to prebuilt menu.\e[0m\n"
        ;;
      3)
        clear_screen
        print_header
        print_description
        if run_prebuilt_flow 3; then
          return 0
        fi
        printf "\n\e[33mPrebuilt install failed. Returning to prebuilt menu.\e[0m\n"
        ;;
      4)
        clear_screen
        print_header
        print_description
        if run_prebuilt_flow 4; then
          return 0
        fi
        printf "\n\e[33mPrebuilt install failed. Returning to prebuilt menu.\e[0m\n"
        ;;
      5)
        clear_screen
        return 1
        ;;
      *)
        printf "\e[31mInvalid option. Please choose 1, 2, 3, 4, or 5.\e[0m\n"
        ;;
    esac
  done
}

select_install_mode() {
  while true; do
    clear_screen
    print_header
    print_description
    printf "\n\e[36mChoose how you want to install NeuroPipe:\e[0m\n"

    select _choice in "Build from source" "Use prebuilt binaries" "Exit"; do
      case "${REPLY}" in
        1)
          if select_build_targets; then
            return 0
          else
            break
          fi
          ;;
        2)
          if select_prebuilt_targets; then
            return 0
          else
            break
          fi
          ;;
        3)
          clear_screen
          printf "\n\e[32mExiting installer.\e[0m\n"
          exit 0
          ;;
        *)
          printf "\e[31mInvalid option. Please choose 1, 2, or 3.\e[0m\n"
          ;;
      esac
    done
  done
}

main() {
  select_install_mode
}

main "$@"

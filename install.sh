#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

TTS_DIR="$ROOT_DIR/tts-service"
STT_DIR="$ROOT_DIR/stt-service"

declare -a SERVICE_UNITS=()

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

verify_prerequisites() {
  require_command uv
  require_command systemctl
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

prompt_install_approval() {
  printf "\n\e[36mBuild artifacts are ready.\e[0m\n"
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

run_build_flow() {
  local selection="$1"

  verify_prerequisites
  SERVICE_UNITS=()

  case "$selection" in
    1)
      build_component "TTS Service" "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      ;;
    2)
      build_component "STT Service" "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    3)
      build_component "TTS Service" "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      build_component "STT Service" "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    *)
      printf "\e[31mUnknown build selection: %s\e[0m\n" "$selection"
      return 1
      ;;
  esac

  prompt_install_approval
  prepare_install_dirs

  case "$selection" in
    1)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      ;;
    2)
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
    3)
      install_component_files "$TTS_DIR" "neuro-tts-service" "neuropipe-tts.service"
      install_component_files "$STT_DIR" "neuro-stt-service" "neuropipe-stt.service"
      ;;
  esac

  enable_and_start_services

  printf "\n\e[32mBuild and installation finished successfully.\e[0m\n"
}

select_build_targets() {
  clear_screen
  print_header
  print_description
  printf "\n\e[36mWhat would you like to build?\e[0m\n"

  select _choice in "Build TTS" "Build STT" "Build both TTS and STT" "Back"; do
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
        return 1
        ;;
      *)
        printf "\e[31mInvalid option. Please choose 1, 2, 3, or 4.\e[0m\n"
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
          clear_screen
          print_header
          printf "\n\e[33mSelected: Use prebuilt binaries\e[0m\n"
          printf "\e[33mTODO: Prebuilt binary install flow will be added next.\e[0m\n"
          return 0
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

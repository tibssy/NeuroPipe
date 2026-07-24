use clap::{Parser, Subcommand};

mod zmq_client;
mod tts;
mod stt;
mod assistant;
mod status;

#[derive(Parser)]
#[command(name = "neuro-ipc", about = "NeuroPipe IPC CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Tts(Tts),
    Stt(Stt),
    Assistant(Assistant),
    Status,
}

#[derive(clap::Args)]
struct Tts {
    #[command(subcommand)]
    action: TtsAction,
}

#[derive(Subcommand)]
enum TtsAction {
    Speak {
        text: String,
        #[arg(long)]
        voice: Option<String>,
        #[arg(long)]
        speed: Option<f64>,
        #[arg(long, value_parser = ["low", "high"])]
        quality: Option<String>,
        #[arg(long, value_parser = ["kokoro", "pocket-tts"])]
        engine: Option<String>,
        #[arg(long)]
        no_monitor: bool,
    },
    Stop,
    GetState,
    SetState {
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        voice: Option<String>,
        #[arg(long)]
        speed: Option<f64>,
        #[arg(long, value_parser = ["low", "high"])]
        quality: Option<String>,
    },
    Monitor,
}

#[derive(clap::Args)]
struct Stt {
    #[command(subcommand)]
    action: SttAction,
}

#[derive(Subcommand)]
enum SttAction {
    Trigger,
    Vad,
    Idle,
    RecordStart,
    RecordStop,
    Listen,
}

#[derive(clap::Args)]
struct Assistant {
    #[command(subcommand)]
    action: AssistantAction,
}

#[derive(Subcommand)]
enum AssistantAction {
    Mode1 {
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        voice: Option<String>,
    },
    Mode2 {
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        voice: Option<String>,
    },
    Interrupt,
    Stop,
    GetState,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tts(t) => match t.action {
            TtsAction::Speak { text, voice, speed, quality, engine, no_monitor } => {
                tts::speak(&text, voice.as_deref(), speed, quality.as_deref(), engine.as_deref(), !no_monitor);
            }
            TtsAction::Stop => tts::stop(),
            TtsAction::GetState => tts::get_state(),
            TtsAction::SetState { engine, voice, speed, quality } => {
                tts::set_state(engine.as_deref(), voice.as_deref(), speed, quality.as_deref());
            }
            TtsAction::Monitor => tts::monitor(),
        },
        Commands::Stt(s) => match s.action {
            SttAction::Trigger => stt::trigger(),
            SttAction::Vad => stt::vad(),
            SttAction::Idle => stt::idle(),
            SttAction::RecordStart => stt::record_start(),
            SttAction::RecordStop => stt::record_stop(),
            SttAction::Listen => stt::listen(),
        },
        Commands::Assistant(a) => match a.action {
            AssistantAction::Mode1 { model, engine, voice } => {
                assistant::start("mode1", model.as_deref(), engine.as_deref(), voice.as_deref());
            }
            AssistantAction::Mode2 { model, engine, voice } => {
                assistant::start("mode2", model.as_deref(), engine.as_deref(), voice.as_deref());
            }
            AssistantAction::Interrupt => assistant::interrupt(),
            AssistantAction::Stop => assistant::stop(),
            AssistantAction::GetState => assistant::get_state(),
        },
        Commands::Status => status::status(),
    }
}

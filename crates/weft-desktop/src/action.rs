//! What a click or a key asks the desktop to do, by the part of the window it
//! belongs to. Projects are named by their folder's full path.

use std::path::PathBuf;

use weft_core::state::PaneViewMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Window(WindowAct),
    Tree(TreeAct),
    Bar(BarAct),
    Sheet(SheetAct),
    Pane(PaneAct),
}

/// Where the keyboard goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
    /// The agent in front's terminal.
    #[default]
    Terminal,
    Search,
    /// The agent in front's message field.
    Message,
    /// The open Ask's field.
    Ask,
    /// A setting typed in Settings (a role's model).
    Setting,
}

/// The top bar, `☰` and the window itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowAct {
    ToggleSidebar,
    ToggleMenu,
    ClosePopovers,
    ToggleTurbo,
    ToggleStartAtLogin,
    /// Pick a folder to open as a project.
    OpenProject,
    OpenUpdates,
    OpenRemote,
    OpenSettings,
    /// ■ NEEDS YOU: the next agent waiting.
    NextNeedsYou,
    RequestQuit,
    Focus(Focus),
}

/// Settings' tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Workflow,
    RingFrame,
    Harnesses,
}

/// A switch in Settings that only the machine has (Turbo and Start at login
/// are ☰'s too, so they are its acts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Notify,
    Suggest,
}

/// The tree's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAct {
    SelectProject(PathBuf),
    ToggleProject(PathBuf),
    CloseProject(PathBuf),
    /// The ■ on a waiting agent's row: why it waits.
    WhyWaiting {
        project: PathBuf,
        session: String,
    },
    TogglePlus(PathBuf),
    /// A project row's ⋯: its settings, and closing it.
    ToggleProjectMenu(PathBuf),
    OpenProjectSettings(PathBuf),
    StartAgent {
        project: PathBuf,
        harness: String,
    },
    /// An agent, by its session, and its pane while one shows it.
    SelectAgent {
        project: PathBuf,
        session: String,
        pane: Option<u32>,
    },
    OpenEarlier(PathBuf),
    /// A unit of work, by its root Ask: its Details open.
    SelectWork {
        project: PathBuf,
        unit: String,
    },
    /// An Ask's own Details, from its unit's workflow.
    SelectAsk {
        project: PathBuf,
        unit: String,
    },
    SelectEval {
        project: PathBuf,
        eval: String,
    },
    /// The sealed units WORK folds: listed, or folded again.
    ToggleSealed(PathBuf),
    SelectSeal {
        project: PathBuf,
        seal: String,
    },
    ClearSearch,
}

/// How a staged prompt is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    No,
    Yes,
    /// Yes, though the agent has not said it is ready.
    Anyway,
}

/// The bar along the window's foot, and what it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarAct {
    Ask,
    Eval,
    Seal,
    Cancel,
    Resolve {
        project: PathBuf,
        pending: String,
        answer: Answer,
    },
    PickDisposition {
        project: PathBuf,
        eval: String,
        disposition: String,
    },
    ConfirmSeal {
        project: PathBuf,
        eval: String,
        disposition: String,
    },
    LeaveRunningAndQuit,
    StopAgentsAndQuit,
    AllowDevice {
        device: String,
        yes: bool,
    },
    ConfirmCloseProject(PathBuf),
    /// Where a staged send goes instead of where it was staged.
    SendTo {
        pending: String,
        to: String,
    },
}

/// The sheets that take the workspace's place, and the docked Ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetAct {
    Close,
    /// Settings shows another of its tabs.
    SettingsTab(SettingsTab),
    /// A switch in Settings turned: the machine's, through the daemon.
    Toggle(Setting),
    /// A setting's choices shown, or hidden again, by its key.
    OpenChoice(Vec<String>),
    /// A setting chosen: every project's, or `project`'s own; `None` takes
    /// it out.
    Choose {
        project: Option<PathBuf>,
        key: Vec<String>,
        value: Option<serde_json::Value>,
    },
    /// A setting typed: its field takes the keyboard.
    EditSetting {
        project: Option<PathBuf>,
        key: Vec<String>,
    },
    /// The typed setting kept (Enter); empty takes it out.
    CommitSetting,
    /// "+ Change a setting here": what a project can set for itself.
    AddProjectSetting,
    /// The preview's host, route or task picked (`0`, `1`, `2`).
    PreviewPick(usize, String),
    /// The preview shown, as RingFrame writes it.
    ShowPreview,
    /// Workflow's "Each judge" opened, or closed again.
    ShowJudges,
    /// How hard Eval's judges look: `quick`, `balanced` or `thorough`, which
    /// sets all three judges' efforts.
    HowHard(String),
    /// A harness's details shown in Settings › Harnesses, by its name.
    ShowHarness(String),
    /// Settings › RingFrame shows every rule, not the first few.
    AllRules,
    /// SEND TO opened or closed.
    ToggleSendTo,
    /// Where the open Ask goes: an agent's session, or a new agent's key.
    AskTo(String),
    SubmitAsk,
    FollowUp {
        project: PathBuf,
        unit: String,
    },
    Proceed {
        project: PathBuf,
        unit: String,
    },
    /// Seal this Eval: the disposition bar opens.
    Seal {
        project: PathBuf,
        eval: String,
    },
    /// Eval this unit of work.
    EvalWork {
        project: PathBuf,
        unit: String,
    },
    /// Seal this unit of work: the disposition bar opens.
    SealWork {
        project: PathBuf,
        unit: String,
    },
    EvalView {
        project: PathBuf,
        eval: String,
    },
    ToggleEvalViewMode,
    /// Open a part, a file, a step or a change of the Eval view, by its key.
    EvalOpen(String),
    /// Back one level of the Eval view; from its top, to the Eval's Details.
    EvalBack,
    /// Back to the Details this one was opened from.
    Back,
    Resume {
        project: PathBuf,
        harness: String,
        session: String,
    },
    SetupAll,
    SetupRun(String),
    Locate(String),
    RemoteOn,
    RemoteOff,
    /// A tunnel picked, not yet used.
    RemotePick(String),
    RemoteUse,
    /// Look for the chosen tunnel's program again, the person having installed it.
    RemoteLook,
    /// A tunnel's installation guide, opened in the browser.
    OpenGuide(String),
    RemoteSignIn,
    RemoteCancel,
    /// Stop the tunnel and choose again.
    RemoteChangeTunnel,
    AskRemoveDevice {
        device: String,
        name: String,
    },
    RemoveDevice(String),
}

/// The panes, their tabs and the message field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAct {
    ToggleLayoutMenu(usize),
    SplitRight(usize),
    SplitDown(usize),
    MoveTabNext(usize),
    JoinAll,
    Focus(usize),
    SelectTab { group: usize, tab: usize },
    DropTab { group: usize, tab: usize, target: crate::panes_view::DropTarget },
    CloseTab { group: usize, tab: usize },
    SetView { group: usize, tab: usize, view: PaneViewMode },
    ToggleSteps { project: PathBuf, pane: u32, block: usize },
    Older { project: PathBuf, pane: u32 },
    TakeSuggestion,
    SendMessage,
}

macro_rules! area {
    ($($t:ident => $v:ident),*) => {$(
        impl From<$t> for Action {
            fn from(a: $t) -> Action {
                Action::$v(a)
            }
        }
    )*};
}
area!(WindowAct => Window, TreeAct => Tree, BarAct => Bar, SheetAct => Sheet, PaneAct => Pane);

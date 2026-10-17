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
    RequestQuit,
    Focus(Focus),
}

/// The tree's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeAct {
    SelectProject(PathBuf),
    ToggleProject(PathBuf),
    CloseProject(PathBuf),
    TogglePlus(PathBuf),
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
    ToggleOpen(PathBuf),
    SelectOpen(PathBuf),
    SelectAsk {
        project: PathBuf,
        unit: String,
    },
    ToggleEval {
        project: PathBuf,
        eval: String,
    },
    SelectEval {
        project: PathBuf,
        eval: String,
    },
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
    Resolve { project: PathBuf, pending: String, answer: Answer },
    PickDisposition { project: PathBuf, eval: String, disposition: String },
    ConfirmSeal { project: PathBuf, eval: String, disposition: String },
    LeaveRunningAndQuit,
    StopAgentsAndQuit,
    AllowDevice { device: String, yes: bool },
}

/// The sheets that take the workspace's place, and the docked Ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetAct {
    Close,
    AskTarget {
        session: String,
    },
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
    EvalView {
        project: PathBuf,
        eval: String,
    },
    ToggleEvalViewMode,
    Resume {
        project: PathBuf,
        harness: String,
        session: String,
    },
    SetupAll,
    SetupRun(String),
    Locate(String),
    RemoteOff,
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
    CloseTab { group: usize, tab: usize },
    SetView { group: usize, tab: usize, view: PaneViewMode },
    ToggleSteps { project: PathBuf, pane: u32, block: usize },
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

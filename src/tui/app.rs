use std::sync::mpsc::Sender;

use crate::domain::multisig::MultisigInfo;
use crate::domain::proposal::{ProposalDetail, ProposalSummary};
use crate::error::MsigError;
use crate::infra::config::Config;

// ---------------------------------------------------------------------------
// Loadable<T> — state wrapper for async data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Loaded(T),
    Failed(String),
}

impl<T> Loadable<T> {
    #[allow(dead_code)]
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    #[allow(dead_code)]
    pub fn as_loaded(&self) -> Option<&T> {
        match self {
            Self::Loaded(v) => Some(v),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Message — Elm-architecture messages from worker or UI events
// ---------------------------------------------------------------------------

pub enum Message {
    // Data loading responses
    MultisigLoaded(Result<MultisigInfo, MsigError>),
    ProposalsLoaded(Result<Vec<ProposalSummary>, MsigError>),
    ProposalDetailLoaded(Result<ProposalDetail, MsigError>),
    TransferCreated(Result<Option<String>, MsigError>),
    ProposalActionCompleted(Result<Option<String>, MsigError>),

    // User-initiated (reserved for future animation/spinner support)
    #[allow(dead_code)]
    Tick,
}

// ---------------------------------------------------------------------------
// RpcRequest — sent to the background worker thread
// ---------------------------------------------------------------------------

#[allow(clippy::enum_variant_names)]
pub enum RpcRequest {
    FetchMultisigInfo {
        addr: String,
        vault_index: u8,
        program_id: solana_pubkey::Pubkey,
    },
    FetchProposals {
        multisig: String,
        limit: u64,
        offset: u64, // skip this many from the end (for pagination)
        program_id: solana_pubkey::Pubkey,
    },
    FetchProposalDetail {
        multisig: String,
        index: u64,
        program_id: solana_pubkey::Pubkey,
    },
    CreateSolTransfer {
        config: Box<Config>,
        ledger: Option<String>,
        multisig: String,
        recipient: String,
        amount_lamports: u64,
        vault_index: u8,
        dry_run: bool,
    },
    RunProposalAction {
        config: Box<Config>,
        ledger: Option<String>,
        multisig: String,
        index: u64,
        action: ProposalAction,
        dry_run: bool,
    },
}

// ---------------------------------------------------------------------------
// Screen — navigation targets
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Setup,
    Selector,
    Dashboard,
    Proposals,
    ProposalDetail { index: u64 },
    Create,
    CommandPalette,
    ConfirmAction,
}

// ---------------------------------------------------------------------------
// ScreenState — per-screen mutable state
// ---------------------------------------------------------------------------

pub struct SelectorState {
    pub input: String,
    pub cursor: usize,
    pub saved_multisigs: Vec<(String, Option<String>)>, // (address, label)
    pub selected_index: usize,
    pub error_msg: Option<String>,
}

pub struct DashboardState {
    pub multisig_info: Loadable<MultisigInfo>,
    pub proposals: Loadable<Vec<ProposalSummary>>,
}

pub struct ProposalsState {
    pub proposals: Loadable<Vec<ProposalSummary>>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub page: u64,      // current page (0-based)
    pub page_size: u64, // proposals per page
}

pub struct ProposalDetailState {
    pub detail: Loadable<ProposalDetail>,
    pub index: u64,
    pub scroll_offset: usize,
    pub action_message: Option<String>,
    pub action_message_is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalAction {
    Approve,
    Reject,
    Cancel,
    Execute,
}

impl ProposalAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::Reject => "Reject",
            Self::Cancel => "Cancel",
            Self::Execute => "Execute",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Self::Approve => "Cast an approval vote on this proposal.",
            Self::Reject => "Reject this active proposal.",
            Self::Cancel => "Cancel this approved proposal.",
            Self::Execute => "Execute the approved transaction.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    OpenProposals,
    CreateSolTransfer,
    SwitchMultisig,
    Refresh,
    ApproveProposal(u64),
    RejectProposal(u64),
    CancelProposal(u64),
    ExecuteProposal(u64),
    Quit,
}

pub struct PaletteEntry {
    pub label: String,
    pub hint: String,
    pub action: PaletteAction,
}

pub struct CommandPaletteState {
    pub entries: Vec<PaletteEntry>,
    pub selected_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmPhase {
    Review,
    Submitting,
    Submitted,
}

pub struct ConfirmActionState {
    pub action: ProposalAction,
    pub proposal_index: u64,
    pub phase: ConfirmPhase,
    pub message: Option<String>,
    pub message_is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatePhase {
    Editing,
    Review,
    Submitting,
    Submitted,
}

pub struct CreateState {
    pub recipient: String,
    pub amount_sol: String,
    pub active_field: usize,
    pub cursor: usize,
    pub phase: CreatePhase,
    pub message: Option<String>,
    pub message_is_error: bool,
}

impl Default for CreateState {
    fn default() -> Self {
        Self {
            recipient: String::new(),
            amount_sol: String::new(),
            active_field: 0,
            cursor: 0,
            phase: CreatePhase::Editing,
            message: None,
            message_is_error: false,
        }
    }
}

pub struct SetupState {
    pub cluster: String,
    pub keypair: String,
    pub multisig: String,
    pub active_field: usize, // 0=cluster, 1=keypair, 2=multisig
    pub cursor: usize,
    pub message: Option<String>,
    pub message_is_error: bool,
}

pub enum ScreenState {
    Setup(SetupState),
    Selector(SelectorState),
    Dashboard(DashboardState),
    Proposals(ProposalsState),
    ProposalDetail(ProposalDetailState),
    Create(CreateState),
    CommandPalette(CommandPaletteState),
    ConfirmAction(ConfirmActionState),
}

impl ScreenState {
    pub fn screen_type(&self) -> Screen {
        match self {
            Self::Setup(_) => Screen::Setup,
            Self::Selector(_) => Screen::Selector,
            Self::Dashboard(_) => Screen::Dashboard,
            Self::Proposals(_) => Screen::Proposals,
            Self::ProposalDetail(s) => Screen::ProposalDetail { index: s.index },
            Self::Create(_) => Screen::Create,
            Self::CommandPalette(_) => Screen::CommandPalette,
            Self::ConfirmAction(_) => Screen::ConfirmAction,
        }
    }
}

// ---------------------------------------------------------------------------
// App — top-level application state
// ---------------------------------------------------------------------------

pub struct App {
    pub screen_stack: Vec<ScreenState>,
    pub config: Config,
    pub should_quit: bool,
    pub multisig_address: Option<String>,
    pub ledger: Option<String>,
    pub dry_run: bool,
}

impl App {
    #[allow(dead_code)]
    pub fn new(config: Config) -> Self {
        Self::with_options(config, None, false)
    }

    pub fn with_options(config: Config, ledger: Option<String>, dry_run: bool) -> Self {
        let saved_multisigs = build_saved_multisigs(&config);
        let needs_setup =
            config.keypair.is_none() && config.multisig.is_none() && !config_file_exists();

        let initial_screen = if needs_setup {
            // First time — guide through setup
            ScreenState::Setup(SetupState {
                cluster: "mainnet".to_string(),
                keypair: String::new(),
                multisig: String::new(),
                active_field: 0,
                cursor: 7, // after "mainnet"
                message: None,
                message_is_error: false,
            })
        } else if config.multisig.is_some() {
            // Configured — go straight to dashboard
            ScreenState::Dashboard(DashboardState {
                multisig_info: Loadable::Loading,
                proposals: Loadable::Loading,
            })
        } else {
            // Config exists but no multisig set — selector
            ScreenState::Selector(SelectorState {
                input: String::new(),
                cursor: 0,
                saved_multisigs,
                selected_index: 0,
                error_msg: None,
            })
        };

        Self {
            multisig_address: config.multisig.clone(),
            screen_stack: vec![initial_screen],
            config,
            should_quit: false,
            ledger,
            dry_run,
        }
    }

    /// Trigger initial data loads if we start on the dashboard.
    pub fn init(&self, request_tx: &Sender<RpcRequest>) {
        if let Some(ref addr) = self.multisig_address {
            let program_id = self.config.program_id;
            let _ = request_tx.send(RpcRequest::FetchMultisigInfo {
                addr: addr.clone(),
                vault_index: self.config.vault_index,
                program_id,
            });
            let _ = request_tx.send(RpcRequest::FetchProposals {
                multisig: addr.clone(),
                limit: 20,
                offset: 0,
                program_id,
            });
        }
    }

    pub fn push_screen(&mut self, screen: ScreenState) {
        self.screen_stack.push(screen);
    }

    pub fn pop_screen(&mut self) {
        if self.screen_stack.len() > 1 {
            self.screen_stack.pop();
        }
    }

    /// Returns a reference to the current (topmost) screen.
    /// The screen stack is guaranteed to always have at least one element.
    pub fn current_screen(&self) -> &ScreenState {
        // Safety: stack is initialized with one element and pop_screen guards len > 1
        let len = self.screen_stack.len();
        &self.screen_stack[len - 1]
    }

    pub fn current_screen_mut(&mut self) -> &mut ScreenState {
        let len = self.screen_stack.len();
        &mut self.screen_stack[len - 1]
    }

    // -- Update (Elm architecture) ----------------------------------------

    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::MultisigLoaded(result) => {
                if let ScreenState::Dashboard(ref mut state) = self.current_screen_mut() {
                    match result {
                        Ok(info) => state.multisig_info = Loadable::Loaded(info),
                        Err(e) => state.multisig_info = Loadable::Failed(e.to_string()),
                    }
                }
            }
            Message::ProposalsLoaded(result) => match self.current_screen_mut() {
                ScreenState::Dashboard(ref mut state) => match result {
                    Ok(proposals) => state.proposals = Loadable::Loaded(proposals),
                    Err(e) => state.proposals = Loadable::Failed(e.to_string()),
                },
                ScreenState::Proposals(ref mut state) => match result {
                    Ok(proposals) => state.proposals = Loadable::Loaded(proposals),
                    Err(e) => state.proposals = Loadable::Failed(e.to_string()),
                },
                _ => {}
            },
            Message::ProposalDetailLoaded(result) => {
                if let ScreenState::ProposalDetail(ref mut state) = self.current_screen_mut() {
                    match result {
                        Ok(detail) => state.detail = Loadable::Loaded(detail),
                        Err(e) => state.detail = Loadable::Failed(e.to_string()),
                    }
                }
            }
            Message::TransferCreated(result) => {
                if let ScreenState::Create(ref mut state) = self.current_screen_mut() {
                    state.phase = CreatePhase::Submitted;
                    match result {
                        Ok(Some(sig)) => {
                            state.message = Some(format!("Proposal transaction sent: {sig}"));
                            state.message_is_error = false;
                        }
                        Ok(None) => {
                            state.message =
                                Some("Dry run succeeded; nothing was sent.".to_string());
                            state.message_is_error = false;
                        }
                        Err(e) => {
                            state.message = Some(e.to_string());
                            state.message_is_error = true;
                        }
                    }
                }
            }
            Message::ProposalActionCompleted(result) => {
                if let ScreenState::ConfirmAction(ref mut state) = self.current_screen_mut() {
                    state.phase = ConfirmPhase::Submitted;
                    match result {
                        Ok(Some(sig)) => {
                            state.message = Some(format!("Transaction sent: {sig}"));
                            state.message_is_error = false;
                        }
                        Ok(None) => {
                            state.message =
                                Some("Dry run succeeded; nothing was sent.".to_string());
                            state.message_is_error = false;
                        }
                        Err(e) => {
                            state.message = Some(e.to_string());
                            state.message_is_error = true;
                        }
                    }
                }
            }
            Message::Tick => {
                // Could drive animations or spinners in future
            }
        }
    }

    // -- Key handling -----------------------------------------------------

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent, request_tx: &Sender<RpcRequest>) {
        use crossterm::event::{KeyCode, KeyModifiers};

        // Ctrl+C always quits
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if key.code == KeyCode::Char(' ')
            && !matches!(
                self.current_screen(),
                ScreenState::Setup(_)
                    | ScreenState::Selector(_)
                    | ScreenState::Create(_)
                    | ScreenState::CommandPalette(_)
                    | ScreenState::ConfirmAction(_)
            )
        {
            self.open_command_palette();
            return;
        }

        // Global back navigation
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace)
            && !matches!(
                self.current_screen(),
                ScreenState::Selector(_) | ScreenState::Setup(_)
            )
        {
            // Don't go back from Selector on Backspace if typing
            if key.code == KeyCode::Backspace
                && matches!(self.current_screen(), ScreenState::Dashboard(_))
            {
                // Backspace on dashboard does nothing special
            } else if key.code == KeyCode::Esc {
                self.pop_screen();
                return;
            }
        }

        // Delegate to screen-specific handlers
        match self.current_screen().screen_type() {
            Screen::Setup => self.handle_setup_key(key, request_tx),
            Screen::Selector => self.handle_selector_key(key, request_tx),
            Screen::Dashboard => self.handle_dashboard_key(key, request_tx),
            Screen::Proposals => self.handle_proposals_key(key, request_tx),
            Screen::ProposalDetail { .. } => self.handle_proposal_detail_key(key),
            Screen::Create => self.handle_create_key(key, request_tx),
            Screen::CommandPalette => self.handle_command_palette_key(key, request_tx),
            Screen::ConfirmAction => self.handle_confirm_action_key(key, request_tx),
        }
    }

    fn handle_setup_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        if let ScreenState::Setup(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Tab | KeyCode::Down => {
                    // Move to next field
                    state.active_field = (state.active_field + 1) % 3;
                    state.cursor = match state.active_field {
                        0 => state.cluster.len(),
                        1 => state.keypair.len(),
                        2 => state.multisig.len(),
                        _ => 0,
                    };
                }
                KeyCode::BackTab | KeyCode::Up => {
                    state.active_field = if state.active_field == 0 {
                        2
                    } else {
                        state.active_field - 1
                    };
                    state.cursor = match state.active_field {
                        0 => state.cluster.len(),
                        1 => state.keypair.len(),
                        2 => state.multisig.len(),
                        _ => 0,
                    };
                }
                KeyCode::Char(c) => {
                    let field = match state.active_field {
                        0 => &mut state.cluster,
                        1 => &mut state.keypair,
                        2 => &mut state.multisig,
                        _ => return,
                    };
                    field.insert(state.cursor, c);
                    state.cursor += c.len_utf8();
                    state.message = None;
                }
                KeyCode::Backspace => {
                    let field = match state.active_field {
                        0 => &mut state.cluster,
                        1 => &mut state.keypair,
                        2 => &mut state.multisig,
                        _ => return,
                    };
                    if state.cursor > 0 {
                        // Find the previous char boundary
                        let prev = field[..state.cursor]
                            .char_indices()
                            .last()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        field.remove(prev);
                        state.cursor = prev;
                    }
                    state.message = None;
                }
                KeyCode::Enter => {
                    // Save config and proceed
                    let cluster = state.cluster.trim().to_string();
                    let keypair = state.keypair.trim().to_string();
                    let multisig = state.multisig.trim().to_string();

                    // Save non-empty values
                    if !cluster.is_empty() {
                        let _ = crate::infra::config::file::save_config_value("cluster", &cluster);
                    }
                    if !keypair.is_empty() {
                        let _ = crate::infra::config::file::save_config_value("keypair", &keypair);
                    }
                    if !multisig.is_empty() {
                        let _ =
                            crate::infra::config::file::save_config_value("multisig", &multisig);
                    }

                    // Update in-memory config
                    if !cluster.is_empty() {
                        self.config.cluster =
                            crate::infra::config::file::resolve_cluster_moniker(&cluster);
                    }
                    if !keypair.is_empty() {
                        self.config.keypair = Some(keypair);
                    }
                    if !multisig.is_empty() {
                        self.config.multisig = Some(multisig.clone());
                        self.multisig_address = Some(multisig.clone());
                        // Go to dashboard
                        self.navigate_to_dashboard(&multisig, request_tx);
                        return;
                    }

                    // No multisig — go to selector
                    let saved = build_saved_multisigs(&self.config);
                    self.push_screen(ScreenState::Selector(SelectorState {
                        input: String::new(),
                        cursor: 0,
                        saved_multisigs: saved,
                        selected_index: 0,
                        error_msg: None,
                    }));
                }
                KeyCode::Esc => {
                    // Skip setup — go to selector
                    let saved = build_saved_multisigs(&self.config);
                    self.push_screen(ScreenState::Selector(SelectorState {
                        input: String::new(),
                        cursor: 0,
                        saved_multisigs: saved,
                        selected_index: 0,
                        error_msg: None,
                    }));
                }
                _ => {}
            }
        }
    }

    fn handle_selector_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        if let ScreenState::Selector(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Up if state.selected_index > 0 => {
                    state.selected_index -= 1;
                }
                KeyCode::Down
                    if !state.saved_multisigs.is_empty()
                        && state.selected_index < state.saved_multisigs.len() - 1 =>
                {
                    state.selected_index += 1;
                }
                KeyCode::Char(c) => {
                    // j/k navigate when input is empty, otherwise type into input
                    if state.input.is_empty() && c == 'k' {
                        if state.selected_index > 0 {
                            state.selected_index -= 1;
                        }
                    } else if state.input.is_empty() && c == 'j' {
                        if !state.saved_multisigs.is_empty()
                            && state.selected_index < state.saved_multisigs.len() - 1
                        {
                            state.selected_index += 1;
                        }
                    } else {
                        state.input.insert(state.cursor, c);
                        state.cursor += 1;
                        state.error_msg = None;
                    }
                }
                KeyCode::Backspace => {
                    if state.cursor > 0 {
                        state.cursor -= 1;
                        state.input.remove(state.cursor);
                    }
                    state.error_msg = None;
                }
                KeyCode::Enter => {
                    let addr = if state.input.is_empty() {
                        // Select from saved list
                        state
                            .saved_multisigs
                            .get(state.selected_index)
                            .map(|(a, _)| a.clone())
                    } else {
                        Some(state.input.clone())
                    };

                    if let Some(addr) = addr {
                        // Validate it looks like a pubkey (32-44 base58 chars)
                        if addr.len() < 32 || addr.len() > 44 {
                            state.error_msg =
                                Some("Invalid address: must be 32-44 characters".to_string());
                            return;
                        }
                        self.navigate_to_dashboard(&addr, request_tx);
                    }
                }
                KeyCode::Esc => {
                    if !state.input.is_empty() {
                        state.input.clear();
                        state.cursor = 0;
                    } else {
                        self.should_quit = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_dashboard_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
            }
            KeyCode::Char('p') => {
                // Navigate to proposals list
                self.push_screen(ScreenState::Proposals(ProposalsState {
                    proposals: Loadable::Loading,
                    selected_index: 0,
                    scroll_offset: 0,
                    page: 0,
                    page_size: 20,
                }));
                if let Some(ref addr) = self.multisig_address {
                    let _ = request_tx.send(RpcRequest::FetchProposals {
                        multisig: addr.clone(),
                        limit: 20,
                        offset: 0,
                        program_id: self.config.program_id,
                    });
                }
            }
            KeyCode::Char('r') => {
                // Refresh — clone address first to avoid borrow conflict
                let addr = self.multisig_address.clone();
                if let Some(addr) = addr {
                    if let ScreenState::Dashboard(ref mut state) = self.current_screen_mut() {
                        state.multisig_info = Loadable::Loading;
                        state.proposals = Loadable::Loading;
                    }
                    let _ = request_tx.send(RpcRequest::FetchMultisigInfo {
                        addr: addr.clone(),
                        vault_index: self.config.vault_index,
                        program_id: self.config.program_id,
                    });
                    let _ = request_tx.send(RpcRequest::FetchProposals {
                        multisig: addr,
                        limit: 20,
                        offset: 0,
                        program_id: self.config.program_id,
                    });
                }
            }
            KeyCode::Char('s') => {
                // Switch multisig — go back to selector
                let saved = build_saved_multisigs(&self.config);
                self.push_screen(ScreenState::Selector(SelectorState {
                    input: String::new(),
                    cursor: 0,
                    saved_multisigs: saved,
                    selected_index: 0,
                    error_msg: None,
                }));
            }
            KeyCode::Char('c') => {
                self.push_screen(ScreenState::Create(CreateState::default()));
            }
            _ => {}
        }
    }

    fn handle_proposals_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        // Clone address before mutable borrow to avoid borrow conflicts
        let addr_clone = self.multisig_address.clone();
        let program_id = self.config.program_id;

        if let ScreenState::Proposals(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Loadable::Loaded(ref proposals) = state.proposals {
                        if !proposals.is_empty() && state.selected_index < proposals.len() - 1 {
                            state.selected_index += 1;
                            // Scroll viewport if needed
                            if state.selected_index >= state.scroll_offset + 20 {
                                state.scroll_offset = state.selected_index.saturating_sub(19);
                            }
                        }
                    }
                }
                KeyCode::Char('k') | KeyCode::Up if state.selected_index > 0 => {
                    state.selected_index -= 1;
                    if state.selected_index < state.scroll_offset {
                        state.scroll_offset = state.selected_index;
                    }
                }
                KeyCode::Enter => {
                    if let Loadable::Loaded(ref proposals) = state.proposals {
                        if let Some(p) = proposals.get(state.selected_index) {
                            let index = p.index;
                            if let Some(ref addr) = addr_clone {
                                let _ = request_tx.send(RpcRequest::FetchProposalDetail {
                                    multisig: addr.clone(),
                                    index,
                                    program_id,
                                });
                            }
                        }
                    }
                }
                KeyCode::Char('r') => {
                    state.proposals = Loadable::Loading;
                    if let Some(ref addr) = addr_clone {
                        let _ = request_tx.send(RpcRequest::FetchProposals {
                            multisig: addr.clone(),
                            limit: state.page_size,
                            offset: state.page * state.page_size,
                            program_id,
                        });
                    }
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    state.page += 1;
                    state.selected_index = 0;
                    state.scroll_offset = 0;
                    state.proposals = Loadable::Loading;
                    if let Some(ref addr) = addr_clone {
                        let _ = request_tx.send(RpcRequest::FetchProposals {
                            multisig: addr.clone(),
                            limit: state.page_size,
                            offset: state.page * state.page_size,
                            program_id,
                        });
                    }
                }
                KeyCode::Left | KeyCode::Char('h') if state.page > 0 => {
                    state.page -= 1;
                    state.selected_index = 0;
                    state.scroll_offset = 0;
                    state.proposals = Loadable::Loading;
                    if let Some(ref addr) = addr_clone {
                        let _ = request_tx.send(RpcRequest::FetchProposals {
                            multisig: addr.clone(),
                            limit: state.page_size,
                            offset: state.page * state.page_size,
                            program_id,
                        });
                    }
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                _ => {}
            }
        }

        // Handle Enter navigation (deferred from above to avoid borrow issues)
        self.maybe_navigate_to_proposal_detail(key);
    }

    fn maybe_navigate_to_proposal_detail(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        if key.code != KeyCode::Enter {
            return;
        }
        if let ScreenState::Proposals(ref state) = self.current_screen() {
            if let Loadable::Loaded(ref proposals) = state.proposals {
                if let Some(p) = proposals.get(state.selected_index) {
                    let index = p.index;
                    self.push_screen(ScreenState::ProposalDetail(ProposalDetailState {
                        detail: Loadable::Loading,
                        index,
                        scroll_offset: 0,
                        action_message: None,
                        action_message_is_error: false,
                    }));
                }
            }
        }
    }

    fn handle_proposal_detail_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        if let ScreenState::ProposalDetail(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    state.scroll_offset += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.scroll_offset = state.scroll_offset.saturating_sub(1);
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                _ => {}
            }
        }
    }

    fn open_command_palette(&mut self) {
        let entries = self.command_palette_entries();
        self.push_screen(ScreenState::CommandPalette(CommandPaletteState {
            entries,
            selected_index: 0,
        }));
    }

    fn command_palette_entries(&self) -> Vec<PaletteEntry> {
        let mut entries = Vec::new();

        if let ScreenState::ProposalDetail(state) = self.current_screen() {
            if let Loadable::Loaded(detail) = &state.detail {
                if detail.summary.status.is_active() {
                    entries.push(PaletteEntry {
                        label: format!("Approve proposal #{}", state.index),
                        hint: "cast approval vote".to_string(),
                        action: PaletteAction::ApproveProposal(state.index),
                    });
                    entries.push(PaletteEntry {
                        label: format!("Reject proposal #{}", state.index),
                        hint: "cast rejection vote".to_string(),
                        action: PaletteAction::RejectProposal(state.index),
                    });
                }
                if detail.summary.status.is_approved() {
                    entries.push(PaletteEntry {
                        label: format!("Execute proposal #{}", state.index),
                        hint: "submit execution transaction".to_string(),
                        action: PaletteAction::ExecuteProposal(state.index),
                    });
                    entries.push(PaletteEntry {
                        label: format!("Cancel proposal #{}", state.index),
                        hint: "cancel approved proposal".to_string(),
                        action: PaletteAction::CancelProposal(state.index),
                    });
                }
            }
        }

        entries.extend([
            PaletteEntry {
                label: "Open proposals".to_string(),
                hint: "browse recent activity".to_string(),
                action: PaletteAction::OpenProposals,
            },
            PaletteEntry {
                label: "New SOL transfer".to_string(),
                hint: "create a transfer proposal".to_string(),
                action: PaletteAction::CreateSolTransfer,
            },
            PaletteEntry {
                label: "Refresh".to_string(),
                hint: "reload current screen".to_string(),
                action: PaletteAction::Refresh,
            },
            PaletteEntry {
                label: "Switch multisig".to_string(),
                hint: "select another address".to_string(),
                action: PaletteAction::SwitchMultisig,
            },
            PaletteEntry {
                label: "Quit".to_string(),
                hint: "leave the TUI".to_string(),
                action: PaletteAction::Quit,
            },
        ]);

        entries
    }

    fn handle_command_palette_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        let mut selected_action = None;
        if let ScreenState::CommandPalette(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Esc => {
                    self.pop_screen();
                    return;
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('j') | KeyCode::Down
                    if state.selected_index + 1 < state.entries.len() =>
                {
                    state.selected_index += 1;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.selected_index = state.selected_index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    selected_action = state
                        .entries
                        .get(state.selected_index)
                        .map(|entry| entry.action);
                }
                _ => {}
            }
        }

        if let Some(action) = selected_action {
            self.pop_screen();
            self.apply_palette_action(action, request_tx);
        }
    }

    fn apply_palette_action(&mut self, action: PaletteAction, request_tx: &Sender<RpcRequest>) {
        match action {
            PaletteAction::OpenProposals => self.open_proposals(request_tx),
            PaletteAction::CreateSolTransfer => {
                self.push_screen(ScreenState::Create(CreateState::default()));
            }
            PaletteAction::SwitchMultisig => {
                let saved = build_saved_multisigs(&self.config);
                self.push_screen(ScreenState::Selector(SelectorState {
                    input: String::new(),
                    cursor: 0,
                    saved_multisigs: saved,
                    selected_index: 0,
                    error_msg: None,
                }));
            }
            PaletteAction::Refresh => self.refresh_current_screen(request_tx),
            PaletteAction::ApproveProposal(index) => {
                self.push_confirm_action(index, ProposalAction::Approve);
            }
            PaletteAction::RejectProposal(index) => {
                self.push_confirm_action(index, ProposalAction::Reject);
            }
            PaletteAction::CancelProposal(index) => {
                self.push_confirm_action(index, ProposalAction::Cancel);
            }
            PaletteAction::ExecuteProposal(index) => {
                self.push_confirm_action(index, ProposalAction::Execute);
            }
            PaletteAction::Quit => self.should_quit = true,
        }
    }

    fn push_confirm_action(&mut self, proposal_index: u64, action: ProposalAction) {
        self.push_screen(ScreenState::ConfirmAction(ConfirmActionState {
            action,
            proposal_index,
            phase: ConfirmPhase::Review,
            message: None,
            message_is_error: false,
        }));
    }

    fn handle_confirm_action_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        let multisig = self.multisig_address.clone();
        let config = self.config.clone();
        let ledger = self.ledger.clone();
        let dry_run = self.dry_run;
        let mut should_refresh_after_pop = false;

        if let ScreenState::ConfirmAction(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Esc => {
                    self.pop_screen();
                    return;
                }
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Enter => match state.phase {
                    ConfirmPhase::Review => {
                        let Some(multisig) = multisig else {
                            state.message = Some("No multisig selected.".to_string());
                            state.message_is_error = true;
                            return;
                        };
                        state.phase = ConfirmPhase::Submitting;
                        state.message = Some(format!("Submitting {}...", state.action.label()));
                        state.message_is_error = false;
                        let _ = request_tx.send(RpcRequest::RunProposalAction {
                            config: Box::new(config),
                            ledger,
                            multisig,
                            index: state.proposal_index,
                            action: state.action,
                            dry_run,
                        });
                    }
                    ConfirmPhase::Submitting => {}
                    ConfirmPhase::Submitted => {
                        should_refresh_after_pop = true;
                    }
                },
                _ => {}
            }
        }

        if should_refresh_after_pop {
            self.pop_screen();
            self.refresh_current_screen(request_tx);
        }
    }

    fn open_proposals(&mut self, request_tx: &Sender<RpcRequest>) {
        self.push_screen(ScreenState::Proposals(ProposalsState {
            proposals: Loadable::Loading,
            selected_index: 0,
            scroll_offset: 0,
            page: 0,
            page_size: 20,
        }));
        if let Some(ref addr) = self.multisig_address {
            let _ = request_tx.send(RpcRequest::FetchProposals {
                multisig: addr.clone(),
                limit: 20,
                offset: 0,
                program_id: self.config.program_id,
            });
        }
    }

    fn refresh_current_screen(&mut self, request_tx: &Sender<RpcRequest>) {
        let addr = self.multisig_address.clone();
        let program_id = self.config.program_id;
        match self.current_screen_mut() {
            ScreenState::Dashboard(state) => {
                state.multisig_info = Loadable::Loading;
                state.proposals = Loadable::Loading;
                if let Some(addr) = addr {
                    let _ = request_tx.send(RpcRequest::FetchMultisigInfo {
                        addr: addr.clone(),
                        vault_index: self.config.vault_index,
                        program_id,
                    });
                    let _ = request_tx.send(RpcRequest::FetchProposals {
                        multisig: addr,
                        limit: 20,
                        offset: 0,
                        program_id,
                    });
                }
            }
            ScreenState::Proposals(state) => {
                state.proposals = Loadable::Loading;
                if let Some(addr) = addr {
                    let _ = request_tx.send(RpcRequest::FetchProposals {
                        multisig: addr,
                        limit: state.page_size,
                        offset: state.page * state.page_size,
                        program_id,
                    });
                }
            }
            ScreenState::ProposalDetail(state) => {
                state.detail = Loadable::Loading;
                state.action_message = None;
                if let Some(addr) = addr {
                    let _ = request_tx.send(RpcRequest::FetchProposalDetail {
                        multisig: addr,
                        index: state.index,
                        program_id,
                    });
                }
            }
            _ => {}
        }
    }

    fn handle_create_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        request_tx: &Sender<RpcRequest>,
    ) {
        use crossterm::event::KeyCode;

        let multisig = self.multisig_address.clone();
        let config = self.config.clone();
        let vault_index = config.vault_index;
        let ledger = self.ledger.clone();
        let dry_run = self.dry_run;

        if let ScreenState::Create(ref mut state) = self.current_screen_mut() {
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Tab | KeyCode::Down if state.phase == CreatePhase::Editing => {
                    state.active_field = (state.active_field + 1) % 2;
                    state.cursor = create_field_value(state).len();
                }
                KeyCode::BackTab | KeyCode::Up if state.phase == CreatePhase::Editing => {
                    state.active_field = if state.active_field == 0 { 1 } else { 0 };
                    state.cursor = create_field_value(state).len();
                }
                KeyCode::Char('e') => {
                    if matches!(state.phase, CreatePhase::Review | CreatePhase::Submitted) {
                        state.phase = CreatePhase::Editing;
                        state.message = None;
                        state.message_is_error = false;
                        state.cursor = create_field_value(state).len();
                    } else if state.phase == CreatePhase::Editing {
                        insert_create_char(state, 'e');
                    }
                }
                KeyCode::Char(c) if state.phase == CreatePhase::Editing => {
                    insert_create_char(state, c);
                }
                KeyCode::Backspace if state.phase == CreatePhase::Editing => {
                    remove_create_char(state);
                }
                KeyCode::Enter => match state.phase {
                    CreatePhase::Editing => match validate_create_state(state) {
                        Ok(()) => {
                            state.phase = CreatePhase::Review;
                            state.message =
                                Some("Review: Enter submits, e edits, Esc goes back.".to_string());
                            state.message_is_error = false;
                        }
                        Err(msg) => {
                            state.message = Some(msg);
                            state.message_is_error = true;
                        }
                    },
                    CreatePhase::Review => {
                        let Some(multisig) = multisig else {
                            state.message = Some("No multisig selected.".to_string());
                            state.message_is_error = true;
                            return;
                        };
                        let amount_lamports = match crate::infra::config::tokens::parse_human_amount(
                            state.amount_sol.trim(),
                            9,
                        ) {
                            Ok(amount) => amount,
                            Err(e) => {
                                state.message = Some(e.to_string());
                                state.message_is_error = true;
                                return;
                            }
                        };
                        let recipient = state.recipient.trim().to_string();
                        state.phase = CreatePhase::Submitting;
                        state.message = Some("Submitting transfer proposal...".to_string());
                        state.message_is_error = false;
                        let _ = request_tx.send(RpcRequest::CreateSolTransfer {
                            config: Box::new(config),
                            ledger,
                            multisig,
                            recipient,
                            amount_lamports,
                            vault_index,
                            dry_run,
                        });
                    }
                    CreatePhase::Submitting => {}
                    CreatePhase::Submitted => {
                        state.phase = CreatePhase::Editing;
                        state.message = None;
                        state.message_is_error = false;
                    }
                },
                _ => {}
            }
        }
    }

    fn navigate_to_dashboard(&mut self, addr: &str, request_tx: &Sender<RpcRequest>) {
        self.multisig_address = Some(addr.to_string());
        self.push_screen(ScreenState::Dashboard(DashboardState {
            multisig_info: Loadable::Loading,
            proposals: Loadable::Loading,
        }));
        let program_id = self.config.program_id;
        let _ = request_tx.send(RpcRequest::FetchMultisigInfo {
            addr: addr.to_string(),
            vault_index: self.config.vault_index,
            program_id,
        });
        let _ = request_tx.send(RpcRequest::FetchProposals {
            multisig: addr.to_string(),
            limit: 20,
            offset: 0,
            program_id,
        });
    }
}

fn create_field_value(state: &CreateState) -> &str {
    match state.active_field {
        0 => &state.recipient,
        1 => &state.amount_sol,
        _ => "",
    }
}

fn create_field_value_mut(state: &mut CreateState) -> &mut String {
    match state.active_field {
        0 => &mut state.recipient,
        1 => &mut state.amount_sol,
        _ => &mut state.recipient,
    }
}

fn insert_create_char(state: &mut CreateState, c: char) {
    let cursor = state.cursor;
    let field = create_field_value_mut(state);
    if cursor <= field.len() {
        field.insert(cursor, c);
        state.cursor = cursor + c.len_utf8();
    }
    state.message = None;
}

fn remove_create_char(state: &mut CreateState) {
    if state.cursor == 0 {
        return;
    }
    let cursor = state.cursor;
    let field = create_field_value_mut(state);
    if cursor > field.len() {
        state.cursor = field.len();
        return;
    }
    let prev = field[..cursor]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    field.remove(prev);
    state.cursor = prev;
    state.message = None;
}

fn validate_create_state(state: &CreateState) -> Result<(), String> {
    let recipient = state.recipient.trim();
    if recipient.parse::<solana_pubkey::Pubkey>().is_err() {
        return Err("Recipient must be a valid Solana address.".to_string());
    }
    crate::infra::config::tokens::parse_human_amount(state.amount_sol.trim(), 9)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Check if a user config file already exists.
fn config_file_exists() -> bool {
    crate::infra::config::file::user_config_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Build a list of saved multisig addresses from config labels.
fn build_saved_multisigs(config: &Config) -> Vec<(String, Option<String>)> {
    let mut result = Vec::new();

    // If there's a configured multisig, add it first
    if let Some(ref addr) = config.multisig {
        let label = config.labels.get(addr).cloned();
        result.push((addr.clone(), label));
    }

    // Add any labeled addresses that look like multisigs (all labels for now)
    for (addr, label) in &config.labels {
        // Avoid duplicating the main multisig
        if config.multisig.as_ref() == Some(addr) {
            continue;
        }
        result.push((addr.clone(), Some(label.clone())));
    }

    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loadable_default_is_idle() {
        let l: Loadable<String> = Loadable::default();
        assert!(matches!(l, Loadable::Idle));
    }

    #[test]
    fn loadable_is_loading() {
        let l: Loadable<i32> = Loadable::Loading;
        assert!(l.is_loading());
        let l2: Loadable<i32> = Loadable::Loaded(42);
        assert!(!l2.is_loading());
    }

    #[test]
    fn loadable_as_loaded() {
        let l: Loadable<i32> = Loadable::Loaded(42);
        assert_eq!(l.as_loaded(), Some(&42));
        let l2: Loadable<i32> = Loadable::Loading;
        assert_eq!(l2.as_loaded(), None);
    }

    #[test]
    fn app_new_with_multisig_starts_on_dashboard() {
        let config = Config {
            multisig: Some("TestAddr1234567890123456789012345".to_string()),
            ..Config::default()
        };
        let app = App::new(config);
        assert!(matches!(app.current_screen(), ScreenState::Dashboard(_)));
    }

    #[test]
    fn app_new_without_multisig_starts_on_selector() {
        let config = Config {
            keypair: Some("test-keypair.json".to_string()),
            ..Config::default()
        };
        let app = App::new(config);
        assert!(matches!(app.current_screen(), ScreenState::Selector(_)));
    }

    #[test]
    fn app_push_pop_screen() {
        let config = Config::default();
        let mut app = App::new(config);
        assert_eq!(app.screen_stack.len(), 1);

        app.push_screen(ScreenState::Create(CreateState::default()));
        assert_eq!(app.screen_stack.len(), 2);
        assert!(matches!(app.current_screen(), ScreenState::Create(_)));

        app.pop_screen();
        assert_eq!(app.screen_stack.len(), 1);
        // Can't pop below 1
        app.pop_screen();
        assert_eq!(app.screen_stack.len(), 1);
    }

    #[test]
    fn screen_state_type_round_trip() {
        let s = ScreenState::Selector(SelectorState {
            input: String::new(),
            cursor: 0,
            saved_multisigs: vec![],
            selected_index: 0,
            error_msg: None,
        });
        assert_eq!(s.screen_type(), Screen::Selector);

        let s2 = ScreenState::Dashboard(DashboardState {
            multisig_info: Loadable::Idle,
            proposals: Loadable::Idle,
        });
        assert_eq!(s2.screen_type(), Screen::Dashboard);
    }
}

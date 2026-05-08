pub mod app;
pub mod format;
pub mod screens;
pub mod theme;
pub mod widgets;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::GlobalOpts;
use crate::error::MsigError;
use crate::infra::config;
use crate::infra::rpc::SolanaRpcClient;

use app::{App, Message, ProposalAction, RpcRequest, ScreenState};
use theme::Theme;

/// Main entry point for the TUI. Called when `msig tui` is run.
pub fn launch_tui(globals: GlobalOpts) -> Result<(), MsigError> {
    let flags = globals.to_global_flags();
    let cfg = config::load_config(&flags)?;

    // Set up channels
    let (request_tx, request_rx) = mpsc::channel::<RpcRequest>();
    let (result_tx, result_rx) = mpsc::channel::<Message>();

    // Spawn background RPC worker
    let rpc_url = cfg.cluster.clone();
    let rpc_commitment = cfg.commitment.clone();
    std::thread::spawn(move || {
        let client = SolanaRpcClient::with_commitment(&rpc_url, &rpc_commitment);
        worker_loop(&client, &request_rx, &result_tx);
    });

    // Initialize app state
    let mut app = App::with_options(cfg, globals.ledger.clone(), globals.dry_run);
    app.init(&request_tx);

    // Enter alternate screen / raw mode
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let theme = Theme::dark();

    // Main event loop (50ms poll)
    let result = run_loop(&mut terminal, &mut app, &theme, &request_tx, &result_rx);

    // Restore terminal (always, even on error)
    let _ = terminal::disable_raw_mode();
    let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    theme: &Theme,
    request_tx: &mpsc::Sender<RpcRequest>,
    result_rx: &mpsc::Receiver<Message>,
) -> Result<(), MsigError> {
    loop {
        // Draw
        terminal.draw(|frame| {
            let size = frame.area();

            // Layout: main area + status bar
            let chunks = Layout::vertical([
                Constraint::Min(1),    // main content
                Constraint::Length(1), // status bar
            ])
            .split(size);

            // Render current screen
            render_screen(frame, chunks[0], theme, app);

            // Status bar
            let multisig_label = app
                .multisig_address
                .as_ref()
                .and_then(|addr| app.config.labels.get(addr))
                .map(|s| s.as_str());

            widgets::status_bar::render_status_bar(
                frame,
                chunks[1],
                theme,
                app.multisig_address.as_deref(),
                multisig_label,
                &app.config.cluster,
                app.config.keypair.as_deref(),
            );
        })?;

        // Poll for input events (50ms timeout = ~20fps)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key, request_tx);
            }
        }

        // Drain any messages from the worker thread
        while let Ok(msg) = result_rx.try_recv() {
            app.update(msg);
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn render_screen(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    app: &App,
) {
    match app.current_screen() {
        ScreenState::Setup(state) => {
            screens::setup::render_setup(frame, area, theme, state);
        }
        ScreenState::Selector(state) => {
            screens::selector::render_selector(frame, area, theme, state);
        }
        ScreenState::Dashboard(state) => {
            screens::dashboard::render_dashboard(
                frame,
                area,
                theme,
                state,
                app.multisig_address.as_deref(),
            );
        }
        ScreenState::Proposals(state) => {
            screens::proposals::render_proposals(frame, area, theme, state);
        }
        ScreenState::ProposalDetail(state) => {
            screens::proposals::render_proposal_detail(frame, area, theme, state);
        }
        ScreenState::Create(state) => {
            screens::create::render_create(frame, area, theme, state);
        }
        ScreenState::CommandPalette(state) => {
            screens::palette::render_command_palette(frame, area, theme, state);
        }
        ScreenState::ConfirmAction(state) => {
            screens::palette::render_confirm_action(frame, area, theme, state, app);
        }
    }
}

/// Background worker loop — receives RPC requests, sends results back.
fn worker_loop(
    client: &SolanaRpcClient,
    request_rx: &mpsc::Receiver<RpcRequest>,
    result_tx: &mpsc::Sender<Message>,
) {
    use crate::application::inspect;

    while let Ok(request) = request_rx.recv() {
        let response = match request {
            RpcRequest::FetchMultisigInfo {
                addr,
                vault_index,
                program_id,
            } => {
                let result = addr
                    .parse::<solana_pubkey::Pubkey>()
                    .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{addr}'")))
                    .and_then(|pubkey| {
                        inspect::fetch_multisig_info(client, &pubkey, vault_index, &program_id)
                    });
                Message::MultisigLoaded(result)
            }
            RpcRequest::FetchProposals {
                multisig,
                limit,
                offset,
                program_id,
            } => {
                let result = multisig
                    .parse::<solana_pubkey::Pubkey>()
                    .map_err(|_| {
                        MsigError::Usage(format!("invalid multisig address: '{multisig}'"))
                    })
                    .and_then(|pubkey| {
                        inspect::list_proposals_paged(client, &pubkey, limit, offset, &program_id)
                    });
                Message::ProposalsLoaded(result)
            }
            RpcRequest::FetchProposalDetail {
                multisig,
                index,
                program_id,
            } => {
                let result = multisig
                    .parse::<solana_pubkey::Pubkey>()
                    .map_err(|_| {
                        MsigError::Usage(format!("invalid multisig address: '{multisig}'"))
                    })
                    .and_then(|pubkey| {
                        inspect::get_proposal_detail(client, &pubkey, index, &program_id)
                    });
                Message::ProposalDetailLoaded(result)
            }
            RpcRequest::CreateSolTransfer {
                config,
                ledger,
                multisig,
                recipient,
                amount_lamports,
                vault_index,
                dry_run,
            } => {
                let result = create_sol_transfer(
                    client,
                    *config,
                    ledger,
                    multisig,
                    recipient,
                    amount_lamports,
                    vault_index,
                    dry_run,
                );
                Message::TransferCreated(result)
            }
            RpcRequest::RunProposalAction {
                config,
                ledger,
                multisig,
                index,
                action,
                dry_run,
            } => {
                let result =
                    run_proposal_action(client, *config, ledger, multisig, index, action, dry_run);
                Message::ProposalActionCompleted(result)
            }
        };
        if result_tx.send(response).is_err() {
            break;
        }
    }
}

fn run_proposal_action(
    client: &SolanaRpcClient,
    config: crate::infra::config::Config,
    ledger: Option<String>,
    multisig: String,
    index: u64,
    action: ProposalAction,
    dry_run: bool,
) -> Result<Option<String>, MsigError> {
    let signer =
        crate::infra::signer::resolve_signer(ledger.as_deref(), None, config.keypair.as_deref())?;
    let multisig: solana_pubkey::Pubkey = multisig
        .parse()
        .map_err(|_| MsigError::Usage("invalid multisig address".into()))?;
    match action {
        ProposalAction::Approve => crate::application::proposal::create_vote_proposal_quiet(
            client,
            signer.as_ref(),
            &multisig,
            index,
            crate::domain::proposal::Vote::Approve,
            None,
            &config,
            dry_run,
            true,
        ),
        ProposalAction::Reject => crate::application::proposal::create_vote_proposal_quiet(
            client,
            signer.as_ref(),
            &multisig,
            index,
            crate::domain::proposal::Vote::Reject,
            None,
            &config,
            dry_run,
            true,
        ),
        ProposalAction::Cancel => crate::application::proposal::create_vote_proposal_quiet(
            client,
            signer.as_ref(),
            &multisig,
            index,
            crate::domain::proposal::Vote::Cancel,
            None,
            &config,
            dry_run,
            true,
        ),
        ProposalAction::Execute => crate::application::proposal::execute_proposal_quiet(
            client,
            signer.as_ref(),
            &multisig,
            index,
            &config,
            dry_run,
            true,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_sol_transfer(
    client: &SolanaRpcClient,
    config: crate::infra::config::Config,
    ledger: Option<String>,
    multisig: String,
    recipient: String,
    amount_lamports: u64,
    vault_index: u8,
    dry_run: bool,
) -> Result<Option<String>, MsigError> {
    let signer =
        crate::infra::signer::resolve_signer(ledger.as_deref(), None, config.keypair.as_deref())?;
    let multisig: solana_pubkey::Pubkey = multisig
        .parse()
        .map_err(|_| MsigError::Usage("invalid multisig address".into()))?;
    crate::application::transfer::create_transfer_proposal_quiet(
        client,
        signer.as_ref(),
        &multisig,
        amount_lamports,
        "native",
        &recipient,
        vault_index,
        None,
        &config,
        dry_run,
        true,
    )
}

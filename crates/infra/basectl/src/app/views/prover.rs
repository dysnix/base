use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};

use crate::{
    app::{Action, Resources, View},
    commands::COLOR_BASE_BLUE,
    rpc::ProverSnapshot,
    tui::Keybinding,
};

const KEYBINDINGS: &[Keybinding] = &[
    Keybinding { key: "j/k", description: "Navigate rows" },
    Keybinding { key: "n/p", description: "Next/prev page" },
    Keybinding { key: "f", description: "Cycle status filter" },
    Keybinding { key: "Esc", description: "Back to home" },
    Keybinding { key: "?", description: "Toggle help" },
];

const PAGE_SIZE: usize = 20;

const STATUS_FILTERS: &[Option<i32>] = &[None, Some(1), Some(2), Some(3), Some(4)];

const STATUS_LABELS: &[&str] = &["All", "Queued", "InProgress", "Succeeded", "Failed"];

/// ZK prover service proof list view.
#[derive(Debug, Default)]
pub struct ProverView {
    table_state: TableState,
    selected: usize,
    page: usize,
    filter_idx: usize,
    snapshot: Option<ProverSnapshot>,
}

impl ProverView {
    /// Creates a new prover view.
    pub fn new() -> Self {
        Self::default()
    }

    fn visible_proofs(&self) -> &[base_zk_client::ProofSummary] {
        self.snapshot.as_ref().map_or(&[], |s| &s.proofs)
    }

    fn page_count(&self) -> usize {
        let total = self.visible_proofs().len();
        if total == 0 { 1 } else { total.div_ceil(PAGE_SIZE) }
    }

    fn page_slice(&self) -> &[base_zk_client::ProofSummary] {
        let proofs = self.visible_proofs();
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(proofs.len());
        if start >= proofs.len() { &[] } else { &proofs[start..end] }
    }
}

impl View for ProverView {
    fn keybindings(&self) -> &'static [Keybinding] {
        KEYBINDINGS
    }

    fn tick(&mut self, resources: &mut Resources) -> Action {
        resources.prover.poll();
        if let Some(ref snapshot) = resources.prover.snapshot {
            self.snapshot = Some(snapshot.clone());
        }
        Action::None
    }

    fn handle_key(&mut self, key: KeyEvent, _resources: &mut Resources) -> Action {
        let page_items = self.page_slice().len();

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if page_items > 0 {
                    self.selected = (self.selected + 1) % page_items;
                    self.table_state.select(Some(self.selected));
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if page_items > 0 {
                    self.selected = (self.selected + page_items - 1) % page_items;
                    self.table_state.select(Some(self.selected));
                }
            }
            KeyCode::Char('n') => {
                if self.page + 1 < self.page_count() {
                    self.page += 1;
                    self.selected = 0;
                    self.table_state.select(Some(0));
                }
            }
            KeyCode::Char('p') => {
                if self.page > 0 {
                    self.page -= 1;
                    self.selected = 0;
                    self.table_state.select(Some(0));
                }
            }
            KeyCode::Char('f') => {
                self.filter_idx = (self.filter_idx + 1) % STATUS_FILTERS.len();
                self.page = 0;
                self.selected = 0;
                self.table_state.select(Some(0));
            }
            _ => {}
        }

        Action::None
    }

    fn render(&mut self, frame: &mut Frame<'_>, area: Rect, _resources: &Resources) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
            .split(area);

        render_header(frame, chunks[0], &self.snapshot, self.filter_idx);
        render_table(frame, chunks[1], self, self.page);
        render_footer(frame, chunks[2], self.page, self.page_count(), self.filter_idx);
    }
}

fn render_header(
    f: &mut Frame<'_>,
    area: Rect,
    snapshot: &Option<ProverSnapshot>,
    filter_idx: usize,
) {
    let total = snapshot.as_ref().map_or(0, |s| s.total_count);
    let filter_label = STATUS_LABELS[filter_idx];
    let title = format!(" ZK Prover — {total} proofs (filter: {filter_label}) ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_BASE_BLUE));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if snapshot.is_none() {
        let msg = Paragraph::new("Connecting to prover service…")
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, inner);
    }
}

fn render_table(f: &mut Frame<'_>, area: Rect, view: &mut ProverView, _page: usize) {
    let proofs = view.page_slice();

    let header_cells = ["ID", "Block", "Type", "Status", "Created", "Updated"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row<'_>> = proofs
        .iter()
        .map(|p| {
            let id_short = if p.id.len() > 8 { &p.id[..8] } else { &p.id };
            let block_range = format!("{}+{}", p.start_block_number, p.number_of_blocks_to_prove);
            let proof_type = format_proof_type(p.proof_type);
            let status = format_status(p.status);
            let created = format_timestamp(&p.created_at);
            let updated = format_timestamp(&p.updated_at);

            let status_style = status_color(p.status);

            Row::new(vec![
                Cell::from(id_short.to_string()),
                Cell::from(block_range),
                Cell::from(proof_type),
                Cell::from(status).style(status_style),
                Cell::from(created),
                Cell::from(updated),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Length(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_BASE_BLUE)),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray));

    f.render_stateful_widget(table, area, &mut view.table_state);
}

fn render_footer(f: &mut Frame<'_>, area: Rect, page: usize, page_count: usize, filter_idx: usize) {
    let key_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::DarkGray);
    let sep = Span::styled("  │  ", Style::default().fg(Color::DarkGray));

    let filter_label = STATUS_LABELS[filter_idx];

    let spans = vec![
        Span::styled("[j/k]", key_style),
        Span::raw(" "),
        Span::styled("navigate", desc_style),
        sep.clone(),
        Span::styled("[n/p]", key_style),
        Span::raw(" "),
        Span::styled(format!("page {}/{page_count}", page + 1), desc_style),
        sep.clone(),
        Span::styled("[f]", key_style),
        Span::raw(" "),
        Span::styled(format!("filter: {filter_label}"), desc_style),
        sep.clone(),
        Span::styled("[Esc]", key_style),
        Span::raw(" "),
        Span::styled("back", desc_style),
        sep,
        Span::styled("[?]", key_style),
        Span::raw(" "),
        Span::styled("help", desc_style),
    ];

    let footer = Paragraph::new(Line::from(spans));
    f.render_widget(footer, area);
}

const fn format_proof_type(pt: i32) -> &'static str {
    match pt {
        1 => "Core",
        2 | 3 => "Compressed",
        4 => "Groth16",
        _ => "Unknown",
    }
}

const fn format_status(status: i32) -> &'static str {
    match status {
        1 => "Queued",
        2 => "InProgress",
        3 => "Succeeded",
        4 => "Failed",
        _ => "Unknown",
    }
}

fn status_color(status: i32) -> Style {
    match status {
        1 => Style::default().fg(Color::Cyan),
        2 => Style::default().fg(Color::Yellow),
        3 => Style::default().fg(Color::Green),
        4 => Style::default().fg(Color::Red),
        _ => Style::default().fg(Color::DarkGray),
    }
}

fn format_timestamp(ts: &str) -> String {
    if ts.len() >= 16 {
        ts[..16].replace('T', " ")
    } else {
        ts.to_string()
    }
}

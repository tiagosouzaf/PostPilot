//! As pastas de conexões dentro do Object Explorer.
//!
//! O Object Explorer mostra a mesma árvore de pastas do Registered Servers
//! (ou do workspace aberto): cada pasta numa linha, os servidores dela
//! embaixo. Um servidor conectado aparece ali com a árvore de objetos dele;
//! um desconectado, numa linha apagada que conecta com duplo clique. As
//! linhas saem achatadas, uma a uma, para a casca intercalar as das árvores
//! de objetos e desenhar só as visíveis.

use std::sync::Arc;

use eframe::egui;
use egui_phosphor::regular as icon;
use pgstudio_i18n::{tr, trf};

use super::{
    Action, Menu, NodePath, RegisteredPanel, display_name, drop_target, environment_badge,
    environment_name, icon_text,
};
use crate::secrets::SecretStore;

/// Altura das linhas, igual à do Object Explorer.
const ROW_HEIGHT: f32 = 20.0;
/// Recuo por nível, igual ao do Object Explorer.
const INDENT: f32 = 16.0;
/// Largura da seta de abrir e fechar.
const CARET: f32 = 14.0;

/// Uma linha das pastas de conexões.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    /// Uma pasta.
    Group { groups: Vec<usize>, depth: usize },
    /// Um servidor guardado.
    Server { path: NodePath, depth: usize },
}

/// O que o usuário pediu no menu de uma linha; aplicado depois do desenho
/// com [`RegisteredPanel::apply_tree_menu`].
#[derive(Default)]
pub struct TreeMenu(Option<Menu>);

impl RegisteredPanel {
    /// As pastas e os servidores, achatados na ordem da árvore, sem a raiz
    /// (o que está nela fica no primeiro nível) e sem o que está dentro de
    /// pastas fechadas.
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.push_tree_rows(&[], 0, &mut rows);
        rows
    }

    fn push_tree_rows(&self, groups: &[usize], depth: usize, rows: &mut Vec<TreeRow>) {
        let Some(group) = self.servers.group(groups) else {
            return;
        };
        for i in 0..group.groups.len() {
            let mut sub = groups.to_vec();
            sub.push(i);
            let open = !self.collapsed.contains(&sub);
            rows.push(TreeRow::Group {
                groups: sub.clone(),
                depth,
            });
            if open {
                self.push_tree_rows(&sub, depth + 1, rows);
            }
        }
        for i in 0..group.servers.len() {
            rows.push(TreeRow::Server {
                path: NodePath::server(groups, i),
                depth,
            });
        }
    }

    /// As pastas recolhidas, pelo nome de cada nível (para a sessão: os
    /// nomes valem mesmo se a ordem das pastas mudar).
    pub fn collapsed_names(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = self
            .collapsed
            .iter()
            .filter_map(|groups| {
                (1..=groups.len())
                    .map(|n| self.servers.group(&groups[..n]).map(|g| g.name.clone()))
                    .collect()
            })
            .collect();
        out.sort();
        out
    }

    /// Recolhe as pastas com estes nomes (as que ainda existem).
    pub fn collapse_names(&mut self, names: &[Vec<String>]) {
        for path in names {
            let mut groups = Vec::new();
            let mut group = self.servers.group(&[]);
            for name in path {
                let Some(i) = group.and_then(|g| g.groups.iter().position(|s| s.name == *name))
                else {
                    group = None;
                    break;
                };
                groups.push(i);
                group = self.servers.group(&groups);
            }
            if group.is_some() && !groups.is_empty() {
                self.collapsed.insert(groups);
            }
        }
    }

    /// Abre ou fecha uma pasta.
    pub fn set_collapsed(&mut self, groups: &[usize], collapsed: bool) {
        if collapsed {
            self.collapsed.insert(groups.to_vec());
        } else {
            self.collapsed.remove(groups);
        }
    }

    /// Desenha a linha de uma pasta no Object Explorer.
    pub fn show_group_row(
        &mut self,
        ui: &mut egui::Ui,
        groups: &[usize],
        depth: usize,
        menu: &mut TreeMenu,
    ) {
        let Some(group) = self.servers.group(groups) else {
            return;
        };
        let menu = &mut menu.0;
        let expanded = !self.collapsed.contains(groups);
        let here = NodePath::group(groups);
        let name = group.name.clone();
        let description = group.description.clone();
        let color = self.servers.color(groups);
        let own_environment = group
            .environment
            .as_deref()
            .and_then(|n| self.servers.environment(n));
        let mut toggle = false;
        ui.horizontal(|ui| {
            ui.set_height(ROW_HEIGHT);
            ui.add_space(depth as f32 * INDENT);
            let caret = if expanded {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            };
            let arrow = ui.add(
                egui::Button::new(caret)
                    .frame(false)
                    .min_size(egui::vec2(CARET, 0.0)),
            );
            if arrow.clicked() {
                toggle = true;
            }
            let folder = if expanded {
                icon::FOLDER_OPEN
            } else {
                icon::FOLDER
            };
            let text = icon_text(ui, folder, color, display_name(&name));
            let label = ui.add(
                egui::Button::selectable(self.selected.as_ref() == Some(&here), text)
                    .frame_when_inactive(false)
                    .sense(egui::Sense::click_and_drag()),
            );
            label.dnd_set_drag_payload(here.clone());
            drop_target(ui, &label, &here, groups, menu);
            if let Some(e) = &own_environment {
                environment_badge(ui, environment_name(&e.name), e.color);
            }
            let label = if description.is_empty() {
                label
            } else {
                label.on_hover_text(&description)
            };
            if label.clicked() {
                self.selected = Some(here.clone());
            }
            if label.double_clicked() {
                toggle = true;
            }
            label.context_menu(|ui| {
                self.selected = Some(here.clone());
                if ui.button(tr("New Folder...")).clicked() {
                    *menu = Some(Menu::NewGroup(groups.to_vec()));
                    ui.close();
                }
                if ui.button(tr("New Server Registration...")).clicked() {
                    *menu = Some(Menu::NewServer(groups.to_vec()));
                    ui.close();
                }
                ui.separator();
                self.move_to_menu(ui, &here, menu);
                if ui.button(tr("Delete")).clicked() {
                    *menu = Some(Menu::Delete(here.clone()));
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("Properties")).clicked() {
                    *menu = Some(Menu::Edit(here.clone()));
                    ui.close();
                }
            });
        });
        if toggle {
            self.set_collapsed(groups, expanded);
        }
    }

    /// Desenha a linha de um servidor guardado que não está conectado: o
    /// duplo clique conecta.
    pub fn show_server_row(
        &mut self,
        ui: &mut egui::Ui,
        path: &NodePath,
        depth: usize,
        actions: &mut Vec<Action>,
        menu: &mut TreeMenu,
    ) {
        let Some(server) = self.servers.server(path).cloned() else {
            return;
        };
        let menu = &mut menu.0;
        ui.horizontal(|ui| {
            ui.set_height(ROW_HEIGHT);
            // Alinhado com os servidores conectados, que têm a seta.
            ui.add_space(depth as f32 * INDENT + CARET + ui.spacing().item_spacing.x);
            if let Some(e) = self.servers.node_environment(path) {
                environment_badge(ui, environment_name(&e.name), e.color);
            }
            // Apagado: ainda não conectado.
            let text = egui::RichText::new(format!("{} {}", icon::DESKTOP_TOWER, server.name))
                .color(ui.visuals().weak_text_color());
            let label = ui.add(
                egui::Button::selectable(self.selected.as_ref() == Some(path), text)
                    .frame_when_inactive(false)
                    .sense(egui::Sense::click_and_drag()),
            );
            label.dnd_set_drag_payload(path.clone());
            drop_target(ui, &label, path, &path.groups, menu);
            let mut tip = trf!(
                "{server} ({user}): not connected, double-click to connect.",
                server = server.settings.server_name(),
                user = server.settings.user
            );
            if !server.description.is_empty() {
                tip.push('\n');
                tip.push_str(&server.description);
            }
            let label = label.on_hover_text(tip);
            if label.clicked() {
                self.selected = Some(path.clone());
            }
            if label.double_clicked() {
                actions.push(Action::ConnectExplorer(server.settings.clone()));
            }
            label.context_menu(|ui| {
                self.selected = Some(path.clone());
                if ui.button(tr("Connect")).clicked() {
                    actions.push(Action::ConnectExplorer(server.settings.clone()));
                    ui.close();
                }
                if ui.button(tr("New Query")).clicked() {
                    actions.push(Action::NewQuery(server.settings.clone()));
                    ui.close();
                }
                ui.separator();
                self.move_to_menu(ui, path, menu);
                if ui.button(tr("Delete")).clicked() {
                    *menu = Some(Menu::Delete(path.clone()));
                    ui.close();
                }
                ui.separator();
                if ui.button(tr("Properties")).clicked() {
                    *menu = Some(Menu::Edit(path.clone()));
                    ui.close();
                }
            });
        });
    }

    /// Aplica o que foi pedido nos menus das linhas.
    pub fn apply_tree_menu(&mut self, menu: TreeMenu, store: Option<&Arc<dyn SecretStore>>) {
        match menu.0 {
            Some(Menu::NewServer(parent)) => self.new_server(&parent, store),
            Some(Menu::NewGroup(parent)) => self.new_group(&parent),
            Some(Menu::Edit(path)) if path.server.is_some() => self.edit_server(&path, store),
            Some(Menu::Edit(path)) => self.edit_group(&path.groups),
            Some(Menu::Delete(path)) => self.confirm_delete = Some(path),
            Some(Menu::Environments) => self.edit_environments(),
            Some(Menu::Move(from, to)) => {
                let message = self.move_node(&from, &to);
                if !message.is_empty() {
                    self.message = Some(message);
                }
            }
            None => {}
        }
    }
}

use crate::face_pile::FacePile;
use auto_update::AutoUpdateStatus;
use call::{ActiveCall, ParticipantLocation, Room};
use client::{proto::PeerId, Client, ParticipantIndex, User, UserStore};
use gpui::{
    actions, canvas, div, point, px, rems, Action, AnyElement, AppContext, Element, Hsla,
    InteractiveElement, IntoElement, Model, ParentElement, Path, Render,
    StatefulInteractiveElement, Styled, Subscription, View, ViewContext, VisualContext, WeakView,
    WindowBounds,
};
use project::{Project, RepositoryEntry};
use recent_projects::RecentProjects;
use std::sync::Arc;
use theme::{ActiveTheme, PlayerColors};
use ui::{
    h_stack, popover_menu, prelude::*, Avatar, Button, ButtonLike, ButtonStyle, ContextMenu, Icon,
    IconButton, IconElement, Tooltip,
};
use util::ResultExt;
use vcs_menu::{build_branch_list, BranchList, OpenRecent as ToggleVcsMenu};
use workspace::{notifications::NotifyResultExt, Workspace};

const MAX_PROJECT_NAME_LENGTH: usize = 40;
const MAX_BRANCH_NAME_LENGTH: usize = 40;

actions!(
    collab,
    [
        ShareProject,
        UnshareProject,
        ToggleUserMenu,
        ToggleProjectMenu,
        SwitchBranch
    ]
);

pub fn init(cx: &mut AppContext) {
    cx.observe_new_views(|workspace: &mut Workspace, cx| {
        let titlebar_item = cx.build_view(|cx| CollabTitlebarItem::new(workspace, cx));
        workspace.set_titlebar_item(titlebar_item.into(), cx)
    })
    .detach();
    // cx.add_action(CollabTitlebarItem::share_project);
    // cx.add_action(CollabTitlebarItem::unshare_project);
    // cx.add_action(CollabTitlebarItem::toggle_user_menu);
    // cx.add_action(CollabTitlebarItem::toggle_vcs_menu);
    // cx.add_action(CollabTitlebarItem::toggle_project_menu);
}

pub struct CollabTitlebarItem {
    project: Model<Project>,
    user_store: Model<UserStore>,
    client: Arc<Client>,
    workspace: WeakView<Workspace>,
    _subscriptions: Vec<Subscription>,
}

impl Render for CollabTitlebarItem {
    fn render(&mut self, cx: &mut ViewContext<Self>) -> impl Element {
        let room = ActiveCall::global(cx).read(cx).room().cloned();
        let current_user = self.user_store.read(cx).current_user();
        let client = self.client.clone();
        let project_id = self.project.read(cx).remote_id();

        h_stack()
            .id("titlebar")
            .justify_between()
            .w_full()
            .h(rems(1.75))
            // Set a non-scaling min-height here to ensure the titlebar is
            // always at least the height of the traffic lights.
            .min_h(px(32.))
            .map(|this| {
                if matches!(cx.window_bounds(), WindowBounds::Fullscreen) {
                    this.pl_2()
                } else {
                    // Use pixels here instead of a rem-based size because the macOS traffic
                    // lights are a static size, and don't scale with the rest of the UI.
                    this.pl(px(80.))
                }
            })
            .bg(cx.theme().colors().title_bar_background)
            .on_click(|event, cx| {
                if event.up.click_count == 2 {
                    cx.zoom_window();
                }
            })
            // left side
            .child(
                h_stack()
                    .gap_1()
                    .children(self.render_project_host(cx))
                    .child(self.render_project_name(cx))
                    .children(self.render_project_branch(cx))
                    .when_some(
                        current_user.clone().zip(client.peer_id()).zip(room.clone()),
                        |this, ((current_user, peer_id), room)| {
                            let player_colors = cx.theme().players();
                            let room = room.read(cx);
                            let mut remote_participants =
                                room.remote_participants().values().collect::<Vec<_>>();
                            remote_participants.sort_by_key(|p| p.participant_index.0);

                            this.children(self.render_collaborator(
                                &current_user,
                                peer_id,
                                true,
                                room.is_speaking(),
                                room.is_muted(cx),
                                &room,
                                project_id,
                                &current_user,
                            ))
                            .children(
                                remote_participants.iter().filter_map(|collaborator| {
                                    let is_present = project_id.map_or(false, |project_id| {
                                        collaborator.location
                                            == ParticipantLocation::SharedProject { project_id }
                                    });

                                    let face_pile = self.render_collaborator(
                                        &collaborator.user,
                                        collaborator.peer_id,
                                        is_present,
                                        collaborator.speaking,
                                        collaborator.muted,
                                        &room,
                                        project_id,
                                        &current_user,
                                    )?;

                                    Some(
                                        v_stack()
                                            .id(("collaborator", collaborator.user.id))
                                            .child(face_pile)
                                            .child(render_color_ribbon(
                                                collaborator.participant_index,
                                                player_colors,
                                            ))
                                            .cursor_pointer()
                                            .on_click({
                                                let peer_id = collaborator.peer_id;
                                                cx.listener(move |this, _, cx| {
                                                    this.workspace
                                                        .update(cx, |workspace, cx| {
                                                            workspace.follow(peer_id, cx);
                                                        })
                                                        .ok();
                                                })
                                            })
                                            .tooltip({
                                                let login = collaborator.user.github_login.clone();
                                                move |cx| {
                                                    Tooltip::text(format!("Follow {login}"), cx)
                                                }
                                            }),
                                    )
                                }),
                            )
                        },
                    ),
            )
            // right side
            .child(
                h_stack()
                    .gap_1()
                    .pr_1()
                    .when_some(room, |this, room| {
                        let room = room.read(cx);
                        let project = self.project.read(cx);
                        let is_local = project.is_local();
                        let is_shared = is_local && project.is_shared();
                        let is_muted = room.is_muted(cx);
                        let is_deafened = room.is_deafened().unwrap_or(false);
                        let is_screen_sharing = room.is_screen_sharing();

                        this.when(is_local, |this| {
                            this.child(
                                Button::new(
                                    "toggle_sharing",
                                    if is_shared { "Unshare" } else { "Share" },
                                )
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(
                                    move |this, _, cx| {
                                        if is_shared {
                                            this.unshare_project(&Default::default(), cx);
                                        } else {
                                            this.share_project(&Default::default(), cx);
                                        }
                                    },
                                )),
                            )
                        })
                        .child(
                            IconButton::new("leave-call", ui::Icon::Exit)
                                .style(ButtonStyle::Subtle)
                                .icon_size(IconSize::Small)
                                .on_click(move |_, cx| {
                                    ActiveCall::global(cx)
                                        .update(cx, |call, cx| call.hang_up(cx))
                                        .detach_and_log_err(cx);
                                }),
                        )
                        .child(
                            IconButton::new(
                                "mute-microphone",
                                if is_muted {
                                    ui::Icon::MicMute
                                } else {
                                    ui::Icon::Mic
                                },
                            )
                            .style(ButtonStyle::Subtle)
                            .icon_size(IconSize::Small)
                            .selected(is_muted)
                            .on_click(move |_, cx| crate::toggle_mute(&Default::default(), cx)),
                        )
                        .child(
                            IconButton::new(
                                "mute-sound",
                                if is_deafened {
                                    ui::Icon::AudioOff
                                } else {
                                    ui::Icon::AudioOn
                                },
                            )
                            .style(ButtonStyle::Subtle)
                            .icon_size(IconSize::Small)
                            .selected(is_deafened)
                            .tooltip(move |cx| {
                                Tooltip::with_meta("Deafen Audio", None, "Mic will be muted", cx)
                            })
                            .on_click(move |_, cx| crate::toggle_mute(&Default::default(), cx)),
                        )
                        .child(
                            IconButton::new("screen-share", ui::Icon::Screen)
                                .style(ButtonStyle::Subtle)
                                .icon_size(IconSize::Small)
                                .selected(is_screen_sharing)
                                .on_click(move |_, cx| {
                                    crate::toggle_screen_sharing(&Default::default(), cx)
                                }),
                        )
                    })
                    .map(|el| {
                        let status = self.client.status();
                        let status = &*status.borrow();
                        if matches!(status, client::Status::Connected { .. }) {
                            el.child(self.render_user_menu_button(cx))
                        } else {
                            el.children(self.render_connection_status(status, cx))
                                .child(self.render_sign_in_button(cx))
                                .child(self.render_user_menu_button(cx))
                        }
                    }),
            )
    }
}

fn render_color_ribbon(participant_index: ParticipantIndex, colors: &PlayerColors) -> gpui::Canvas {
    let color = colors.color_for_participant(participant_index.0).cursor;
    canvas(move |bounds, cx| {
        let mut path = Path::new(bounds.lower_left());
        let height = bounds.size.height;
        path.curve_to(bounds.origin + point(height, px(0.)), bounds.origin);
        path.line_to(bounds.upper_right() - point(height, px(0.)));
        path.curve_to(bounds.lower_right(), bounds.upper_right());
        path.line_to(bounds.lower_left());
        cx.paint_path(path, color);
    })
    .h_1()
    .w_full()
}

impl CollabTitlebarItem {
    pub fn new(workspace: &Workspace, cx: &mut ViewContext<Self>) -> Self {
        let project = workspace.project().clone();
        let user_store = workspace.app_state().user_store.clone();
        let client = workspace.app_state().client.clone();
        let active_call = ActiveCall::global(cx);
        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.observe(&workspace.weak_handle().upgrade().unwrap(), |_, _, cx| {
                cx.notify()
            }),
        );
        subscriptions.push(cx.observe(&project, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&active_call, |this, _, cx| this.active_call_changed(cx)));
        subscriptions.push(cx.observe_window_activation(Self::window_activation_changed));
        subscriptions.push(cx.observe(&user_store, |_, _, cx| cx.notify()));

        Self {
            workspace: workspace.weak_handle(),
            project,
            user_store,
            client,
            _subscriptions: subscriptions,
        }
    }

    // resolve if you are in a room -> render_project_owner
    // render_project_owner -> resolve if you are in a room -> Option<foo>

    pub fn render_project_host(&self, cx: &mut ViewContext<Self>) -> Option<impl Element> {
        let host = self.project.read(cx).host()?;
        let host = self.user_store.read(cx).get_cached_user(host.user_id)?;
        let participant_index = self
            .user_store
            .read(cx)
            .participant_indices()
            .get(&host.id)?;
        Some(
            div().border().border_color(gpui::red()).child(
                Button::new("project_owner_trigger", host.github_login.clone())
                    .color(Color::Player(participant_index.0))
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .tooltip(move |cx| Tooltip::text("Toggle following", cx)),
            ),
        )
    }

    pub fn render_project_name(&self, cx: &mut ViewContext<Self>) -> impl Element {
        let name = {
            let mut names = self.project.read(cx).visible_worktrees(cx).map(|worktree| {
                let worktree = worktree.read(cx);
                worktree.root_name()
            });

            names.next().unwrap_or("")
        };

        let name = util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH);
        let workspace = self.workspace.clone();
        popover_menu("project_name_trigger")
            .trigger(
                Button::new("project_name_trigger", name)
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .tooltip(move |cx| Tooltip::text("Recent Projects", cx)),
            )
            .menu(move |cx| Some(Self::render_project_popover(workspace.clone(), cx)))
    }

    pub fn render_project_branch(&self, cx: &mut ViewContext<Self>) -> Option<impl Element> {
        let entry = {
            let mut names_and_branches =
                self.project.read(cx).visible_worktrees(cx).map(|worktree| {
                    let worktree = worktree.read(cx);
                    worktree.root_git_entry()
                });

            names_and_branches.next().flatten()
        };
        let workspace = self.workspace.upgrade()?;
        let branch_name = entry
            .as_ref()
            .and_then(RepositoryEntry::branch)
            .map(|branch| util::truncate_and_trailoff(&branch, MAX_BRANCH_NAME_LENGTH))?;
        Some(
            popover_menu("project_branch_trigger")
                .trigger(
                    Button::new("project_branch_trigger", branch_name)
                        .color(Color::Muted)
                        .style(ButtonStyle::Subtle)
                        .label_size(LabelSize::Small)
                        .tooltip(move |cx| {
                            Tooltip::with_meta(
                                "Recent Branches",
                                Some(&ToggleVcsMenu),
                                "Local branches only",
                                cx,
                            )
                        }),
                )
                .menu(move |cx| Self::render_vcs_popover(workspace.clone(), cx)),
        )
    }

    fn render_collaborator(
        &self,
        user: &Arc<User>,
        peer_id: PeerId,
        is_present: bool,
        is_speaking: bool,
        is_muted: bool,
        room: &Room,
        project_id: Option<u64>,
        current_user: &Arc<User>,
    ) -> Option<FacePile> {
        let followers = project_id.map_or(&[] as &[_], |id| room.followers_for(peer_id, id));

        let pile = FacePile::default()
            .child(
                Avatar::new(user.avatar_uri.clone())
                    .grayscale(!is_present)
                    .border_color(if is_speaking {
                        gpui::blue()
                    } else if is_muted {
                        gpui::red()
                    } else {
                        Hsla::default()
                    }),
            )
            .children(followers.iter().filter_map(|follower_peer_id| {
                let follower = room
                    .remote_participants()
                    .values()
                    .find_map(|p| (p.peer_id == *follower_peer_id).then_some(&p.user))
                    .or_else(|| {
                        (self.client.peer_id() == Some(*follower_peer_id)).then_some(current_user)
                    })?
                    .clone();

                Some(Avatar::new(follower.avatar_uri.clone()))
            }));

        Some(pile)
    }

    fn window_activation_changed(&mut self, cx: &mut ViewContext<Self>) {
        let project = if cx.is_window_active() {
            Some(self.project.clone())
        } else {
            None
        };
        ActiveCall::global(cx)
            .update(cx, |call, cx| call.set_location(project.as_ref(), cx))
            .detach_and_log_err(cx);
    }

    fn active_call_changed(&mut self, cx: &mut ViewContext<Self>) {
        cx.notify();
    }

    fn share_project(&mut self, _: &ShareProject, cx: &mut ViewContext<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.share_project(project, cx))
            .detach_and_log_err(cx);
    }

    fn unshare_project(&mut self, _: &UnshareProject, cx: &mut ViewContext<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.unshare_project(project, cx))
            .log_err();
    }

    pub fn render_vcs_popover(
        workspace: View<Workspace>,
        cx: &mut WindowContext<'_>,
    ) -> Option<View<BranchList>> {
        let view = build_branch_list(workspace, cx).log_err()?;
        let focus_handle = view.focus_handle(cx);
        cx.focus(&focus_handle);
        Some(view)
    }

    pub fn render_project_popover(
        workspace: WeakView<Workspace>,
        cx: &mut WindowContext<'_>,
    ) -> View<RecentProjects> {
        let view = RecentProjects::open_popover(workspace, cx);

        let focus_handle = view.focus_handle(cx);
        cx.focus(&focus_handle);
        view
    }

    fn render_connection_status(
        &self,
        status: &client::Status,
        cx: &mut ViewContext<Self>,
    ) -> Option<AnyElement> {
        match status {
            client::Status::ConnectionError
            | client::Status::ConnectionLost
            | client::Status::Reauthenticating { .. }
            | client::Status::Reconnecting { .. }
            | client::Status::ReconnectionError { .. } => Some(
                div()
                    .id("disconnected")
                    .bg(gpui::red()) // todo!() @nate
                    .child(IconElement::new(Icon::Disconnected))
                    .tooltip(|cx| Tooltip::text("Disconnected", cx))
                    .into_any_element(),
            ),
            client::Status::UpgradeRequired => {
                let auto_updater = auto_update::AutoUpdater::get(cx);
                let label = match auto_updater.map(|auto_update| auto_update.read(cx).status()) {
                    Some(AutoUpdateStatus::Updated) => "Please restart Zed to Collaborate",
                    Some(AutoUpdateStatus::Installing)
                    | Some(AutoUpdateStatus::Downloading)
                    | Some(AutoUpdateStatus::Checking) => "Updating...",
                    Some(AutoUpdateStatus::Idle) | Some(AutoUpdateStatus::Errored) | None => {
                        "Please update Zed to Collaborate"
                    }
                };

                Some(
                    div()
                        .bg(gpui::red()) // todo!() @nate
                        .child(Button::new("connection-status", label).on_click(|_, cx| {
                            if let Some(auto_updater) = auto_update::AutoUpdater::get(cx) {
                                if auto_updater.read(cx).status() == AutoUpdateStatus::Updated {
                                    workspace::restart(&Default::default(), cx);
                                    return;
                                }
                            }
                            auto_update::check(&Default::default(), cx);
                        }))
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    pub fn render_sign_in_button(&mut self, _: &mut ViewContext<Self>) -> Button {
        let client = self.client.clone();
        Button::new("sign_in", "Sign in").on_click(move |_, cx| {
            let client = client.clone();
            cx.spawn(move |mut cx| async move {
                client
                    .authenticate_and_connect(true, &cx)
                    .await
                    .notify_async_err(&mut cx);
            })
            .detach();
        })
    }

    pub fn render_user_menu_button(&mut self, cx: &mut ViewContext<Self>) -> impl Element {
        if let Some(user) = self.user_store.read(cx).current_user() {
            popover_menu("user-menu")
                .menu(|cx| {
                    ContextMenu::build(cx, |menu, _| {
                        menu.action("Settings", zed_actions::OpenSettings.boxed_clone())
                            .action("Theme", theme_selector::Toggle.boxed_clone())
                            .separator()
                            .action("Share Feedback", feedback::GiveFeedback.boxed_clone())
                            .action("Sign Out", client::SignOut.boxed_clone())
                    })
                    .into()
                })
                .trigger(
                    ButtonLike::new("user-menu")
                        .child(
                            h_stack()
                                .gap_0p5()
                                .child(Avatar::new(user.avatar_uri.clone()))
                                .child(IconElement::new(Icon::ChevronDown).color(Color::Muted)),
                        )
                        .style(ButtonStyle::Subtle)
                        .tooltip(move |cx| Tooltip::text("Toggle User Menu", cx)),
                )
                .anchor(gpui::AnchorCorner::TopRight)
        } else {
            popover_menu("user-menu")
                .menu(|cx| {
                    ContextMenu::build(cx, |menu, _| {
                        menu.action("Settings", zed_actions::OpenSettings.boxed_clone())
                            .action("Theme", theme_selector::Toggle.boxed_clone())
                            .separator()
                            .action("Share Feedback", feedback::GiveFeedback.boxed_clone())
                    })
                    .into()
                })
                .trigger(
                    ButtonLike::new("user-menu")
                        .child(
                            h_stack()
                                .gap_0p5()
                                .child(IconElement::new(Icon::ChevronDown).color(Color::Muted)),
                        )
                        .style(ButtonStyle::Subtle)
                        .tooltip(move |cx| Tooltip::text("Toggle User Menu", cx)),
                )
        }
    }
}

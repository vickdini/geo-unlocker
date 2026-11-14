//! Screen layout: top bar, sidebar (connection card + Tor/VPN/Cloud lists), map
//! with overlays, session traffic, drawers and dialogs.

use super::icons::{self, country_name};
use super::map::{self, MapCanvas};
use super::style::{self, alpha, Btn, ACCENT, BAD, BOLD, CARD, FAINT, LINE, MUTED, OK, RAISE, SEMIBOLD, SIDE, TEXT, WARN};
use super::{s, App, Message, Overlay, Pending, Tab};
use iced::widget::{
    button, canvas, center, checkbox, column, container, mouse_area, opaque, pick_list, row, scrollable, space, stack, svg, text,
    text_input, Space,
};
use iced::{Alignment, Color, Element, Length, Padding, Point};
use serde_json::Value;

type El<'a> = Element<'a, Message>;

fn t<'a>(s: impl Into<String>) -> iced::widget::Text<'a> {
    text(s.into()).size(14)
}

fn muted<'a>(s: impl Into<String>) -> iced::widget::Text<'a> {
    text(s.into()).size(12).color(MUTED)
}

fn icon_btn<'a>(h: svg::Handle, msg: Message, tip: &'a str) -> El<'a> {
    let _ = tip;
    button(svg(h).width(16).height(16)).padding(8).style(style::btn(Btn::Icon)).on_press(msg).into()
}

fn flag<'a>(cc: &str, w: f32) -> El<'a> {
    match icons::flag(cc) {
        Some(h) => container(svg(h).width(w).height(w * 0.75)).width(w + 4.0).center_x(w + 4.0).into(),
        None => container(svg(icons::globe()).width(w * 0.8).height(w * 0.8)).width(w + 4.0).center_x(w + 4.0).into(),
    }
}

fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b < 1024.0 {
        format!("{b} B")
    } else if b < 1048576.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1073741824.0 {
        format!("{:.1} MB", b / 1048576.0)
    } else {
        format!("{:.2} GB", b / 1073741824.0)
    }
}

fn fmt_rate(b: f32) -> String {
    format!("{}/s", fmt_bytes(b.max(0.0) as u64))
}

fn fmt_dur(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn describe_mode(m: &Value) -> String {
    match m["kind"].as_str() {
        Some("tor") => format!("Tor · {}{}", country_name(&s(m, "country")), if m["system"].as_bool() == Some(false) { " · proxy apps" } else { "" }),
        Some("vpn") => format!("VPN · {}", s(m, "name")),
        Some("cloud") => "Cloud · Azure".into(),
        _ => "Tor · fastest country".into(),
    }
}

pub fn view(app: &App) -> El<'_> {
    if let Some(e) = &app.fatal {
        return center(
            column![text("Geo Unlocker can't start").size(20).font(BOLD), text(e.clone()).color(MUTED)]
                .spacing(10)
                .align_x(Alignment::Center)
                .max_width(520),
        )
        .style(style::panel(style::BG, 0.0, None))
        .into();
    }
    if let Some(w) = &app.waiting {
        return center(
            column![svg(icons::logo()).width(56).height(56), text("Geo Unlocker").size(18).font(BOLD), text(w.clone()).color(MUTED).center()]
                .spacing(10)
                .align_x(Alignment::Center),
        )
        .style(style::panel(style::BG, 0.0, None))
        .into();
    }
    let main = column![topbar(app), row![sidebar(app), stage(app)].height(Length::Fill)];
    let mut layers: Vec<El> = vec![container(main).style(style::panel(style::BG, 0.0, None)).into()];
    if let Some(o) = &app.overlay {
        layers.push(mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill).style(style::scrim)).on_press(Message::CloseOverlay).into());
        layers.push(match o {
            Overlay::Pi => drawer("Raspberry Pi", pi_drawer(app)),
            Overlay::Updates => drawer("About & updates", updates_drawer(app)),
            Overlay::Advanced => drawer_w("Technical details", advanced_drawer(app), 720.0),
            Overlay::AddProfile => dialog(add_profile(app)),
            Overlay::Confirm { title, body, ok, danger, .. } => dialog(
                column![
                    text(title.clone()).size(17).font(BOLD),
                    text(body.clone()).color(MUTED),
                    row![
                        space::horizontal(),
                        button(t("Cancel")).padding([8, 14]).style(style::btn(Btn::Ghost)).on_press(Message::CloseOverlay),
                        button(t(ok.clone()).font(SEMIBOLD))
                            .padding([8, 14])
                            .style(style::btn(if *danger { Btn::Danger } else { Btn::Primary }))
                            .on_press(Message::Confirmed),
                    ]
                    .spacing(8)
                ]
                .spacing(12)
                .into(),
            ),
        });
    }
    stack(layers).into()
}

fn drawer<'a>(title: &'a str, body: El<'a>) -> El<'a> {
    drawer_w(title, body, 390.0)
}

fn drawer_w<'a>(title: &'a str, body: El<'a>, width: f32) -> El<'a> {
    row![
        space::horizontal(),
        opaque(
            container(
                column![
                    row![text(title).size(17).font(BOLD), space::horizontal(), icon_btn(icons::close(), Message::CloseOverlay, "Close")]
                        .align_y(Alignment::Center),
                    scrollable(container(body).padding(Padding { top: 0.0, right: 16.0, bottom: 0.0, left: 0.0 })).height(Length::Fill)
                ]
                .spacing(10)
            )
            .width(width)
            .height(Length::Fill)
            .padding(18)
            .style(style::floating)
        )
    ]
    .into()
}

fn dialog(body: El<'_>) -> El<'_> {
    center(opaque(container(body).width(440).padding(20).style(style::floating))).into()
}

// ---------------------------------------------------------------- top bar
fn topbar(app: &App) -> El<'_> {
    let u = &app.status["update"];
    let ready = u["state"].as_str() == Some("ready");
    let upd_label = if ready {
        format!("Update {} ready", s(u, "latest"))
    } else if u["state"].as_str() == Some("installing") {
        "Updating…".into()
    } else {
        format!("v{}", s(u, "current"))
    };
    let dot = |c: Color| container(Space::new().width(8).height(8)).style(move |_: &iced::Theme| container::Style {
        background: Some(c.into()),
        border: iced::Border { radius: 4.0.into(), ..Default::default() },
        ..Default::default()
    });
    let upd_color = if ready { WARN } else if u["state"].as_str() == Some("error") { BAD } else if u["enabled"].as_bool() == Some(true) { OK } else { FAINT };
    let pi_color = if app.status["pi"]["url"].is_string() { OK } else { FAINT };
    container(
        row![
            svg(icons::logo()).width(24).height(24),
            text("Geo Unlocker").size(15).font(BOLD),
            space::horizontal(),
            button(row![dot(upd_color), t(upd_label)].spacing(8).align_y(Alignment::Center))
                .padding([6, 12])
                .style(style::btn(Btn::Chip))
                .on_press(Message::OpenUpdates),
            button(row![dot(pi_color), t("Raspberry Pi")].spacing(8).align_y(Alignment::Center))
                .padding([6, 12])
                .style(style::btn(Btn::Chip))
                .on_press(Message::OpenPi),
            button(row![svg(icons::code()).width(15).height(15), t("Details").size(13)].spacing(6).align_y(Alignment::Center))
                .padding([6, 12])
                .style(style::btn(Btn::Chip))
                .on_press(Message::OpenAdvanced),
            icon_btn(icons::quit(), Message::Ask(Pending::Quit), "Disconnect and quit"),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([0, 14])
    .height(48)
    .align_y(Alignment::Center)
    .style(style::panel(SIDE, 0.0, Some(LINE)))
    .into()
}

// ---------------------------------------------------------------- sidebar
fn sidebar(app: &App) -> El<'_> {
    let v = app.conn();
    let geo = match v.state {
        "on" => app.exit.as_ref(),
        "off" => app.home.as_ref(),
        _ => None,
    };
    let ip = geo.map(|g| g.ip.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "—".into());
    let where_ = geo.map(|g| [g.city.clone(), if g.country.is_empty() { String::new() } else { country_name(&g.country) }].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let speeds = match app.speeds {
        Some((d, u)) => format!("↓ {}   ↑ {}", fmt_rate(d), fmt_rate(u)),
        None => "↓ —   ↑ —".into(),
    };
    let (label, sub, kind) = if v.state == "off" {
        let last = &app.status["last_mode"];
        ("Quick connect".to_string(), Some(describe_mode(last)), Btn::Primary)
    } else {
        (if v.state == "wait" { "Cancel" } else { "Disconnect" }.to_string(), None, Btn::Outline)
    };
    let mut main_label = column![text(label).size(15).font(BOLD)].align_x(Alignment::Center);
    if let Some(sub) = sub {
        main_label = main_label.push(text(sub).size(11).color(alpha(Color::WHITE, 0.8)));
    }
    let card = container(
        column![
            row![
                text(if v.state == "off" { "Not connected".to_string() } else { v.title.clone() }).size(17).font(BOLD),
                space::horizontal(),
                if v.state == "off" { Element::from(Space::new()) } else { flag(&v.cc, 24.0) },
            ]
            .align_y(Alignment::Center),
            row![text("IP: ").size(13).color(MUTED), text(ip).size(13).font(SEMIBOLD), space::horizontal(), text(where_).size(13).color(MUTED)],
            row![text(if v.state == "off" { "Unprotected".to_string() } else { v.proto.clone() }).size(13).color(MUTED), space::horizontal(), text(speeds).size(13).font(SEMIBOLD)],
            button(center(main_label)).width(Length::Fill).height(46).style(style::btn(kind)).on_press(Message::MainButton),
        ]
        .spacing(6),
    )
    .padding(Padding { top: 18.0, right: 18.0, bottom: 16.0, left: 18.0 })
    .style(style::conn_card(v.state));

    let tab_btn = |label: &'static str, tab: Tab| {
        button(center(text(label).size(14).font(SEMIBOLD))).width(Length::Fill).height(34).style(style::btn(Btn::Tab(app.tab == tab))).on_press(Message::Tab(tab))
    };
    let tabs = container(row![tab_btn("Tor", Tab::Tor), tab_btn("VPN", Tab::Vpn), tab_btn("Cloud", Tab::Cloud)].spacing(3))
        .padding(3)
        .style(style::panel(style::BG, 10.0, None));
    let search = text_input("Search", &app.search).on_input(Message::Search).padding([9, 12]).size(14).style(style::input);
    let body = match app.tab {
        Tab::Tor => tor_panel(app),
        Tab::Vpn => vpn_panel(app),
        Tab::Cloud => cloud_panel(app),
    };
    container(
        column![
            card,
            column![tabs, search].spacing(12).padding([14, 14]),
            scrollable(container(body).padding(Padding { top: 0.0, right: 12.0, bottom: 14.0, left: 14.0 })).height(Length::Fill)
        ]
        .height(Length::Fill),
    )
    .width(380)
    .height(Length::Fill)
    .style(style::panel(SIDE, 0.0, Some(LINE)))
    .into()
}

fn matches(app: &App, hay: &str) -> bool {
    let q = app.search.trim().to_lowercase();
    q.is_empty() || hay.to_lowercase().contains(&q)
}

fn section<'a>(label: &str, right: El<'a>) -> El<'a> {
    row![text(label.to_uppercase()).size(11).color(FAINT), space::horizontal(), right].align_y(Alignment::Center).padding([10, 4]).into()
}

fn note<'a>(content: El<'a>, warn: bool) -> El<'a> {
    container(content).padding([10, 12]).width(Length::Fill).style(style::panel(CARD, 10.0, Some(if warn { alpha(WARN, 0.4) } else { LINE }))).into()
}

fn connected_pill<'a>() -> El<'a> {
    container(text("CONNECTED").size(10).font(BOLD)).padding([2, 7]).style(style::pill(OK)).into()
}

fn list_row<'a>(lead: El<'a>, name: String, sub: String, active: bool, trailing: Vec<El<'a>>) -> El<'a> {
    let mut r = row![lead, column![text(name).size(14).font(SEMIBOLD), text(sub).size(12).color(MUTED)].width(Length::Fill)]
        .spacing(10)
        .align_y(Alignment::Center);
    if active {
        r = r.push(connected_pill());
    }
    for el in trailing {
        r = r.push(el);
    }
    container(r).padding([8, 8]).style(style::row_style(active)).into()
}

fn connect_btn<'a>(msg: Message, active: bool) -> El<'a> {
    button(center(svg(icons::power()).width(15).height(15))).width(32).height(32).style(style::btn(Btn::Connect(active))).on_press(msg).into()
}

fn tor_panel(app: &App) -> El<'_> {
    let m = &app.status["mode"];
    let active = (m["kind"].as_str() == Some("tor")).then(|| s(m, "country"));
    let seg = |label: &'static str, system: bool| {
        button(center(text(label).size(12).font(SEMIBOLD))).width(Length::Fill).height(30).style(style::btn(Btn::Tab(app.tor_system == system))).on_press(Message::TorScope(system))
    };
    let mut col = column![
        container(row![seg("Whole computer", true), seg("Proxy apps only", false)].spacing(4)).padding(3).style(style::panel(style::BG, 9.0, None)),
        muted(if app.tor_system {
            "Every app goes through Tor, including DNS. Only web-style (TCP) traffic works."
        } else {
            "Only apps that follow the system proxy (most browsers) use Tor."
        })
        .size(12)
        .color(FAINT),
    ]
    .spacing(6);
    if app.status["tools"]["tor"].as_bool() == Some(false) {
        col = col.push(note(t("Tor is missing from this installation. Reinstall Geo Unlocker.").into(), true));
    }
    let Some(list) = &app.tor_list else {
        return col.push(muted("Loading countries…")).into();
    };
    col = col.push(section("Exit countries", muted(list.len().to_string()).into()));
    if matches(app, "fastest any country") {
        col = col.push(list_row(
            container(svg(icons::globe()).width(20).height(20)).width(30).center_x(30).into(),
            "Fastest country".into(),
            "Tor picks the best exit".into(),
            active.as_deref() == Some(""),
            vec![connect_btn(Message::TorConnect(String::new()), active.as_deref() == Some(""))],
        ));
    }
    for (cc, relays, mbps) in list {
        let name = country_name(cc);
        if !matches(app, &format!("{name} {cc}")) {
            continue;
        }
        let on = active.as_deref() == Some(cc.as_str());
        let sub = if *relays > 0 { format!("{relays} exit relays · ~{mbps} Mbit/s") } else { "Exit country".into() };
        col = col.push(list_row(flag(cc, 22.0), name, sub, on, vec![connect_btn(Message::TorConnect(cc.clone()), on)]));
    }
    col.spacing(2).into()
}

fn vpn_panel(app: &App) -> El<'_> {
    let m = &app.status["mode"];
    let active = (m["kind"].as_str() == Some("vpn")).then(|| s(m, "name"));
    let pi = app.status["pi"]["url"].is_string();
    let add = button(row![svg(icons::plus()).width(14).height(14), t("Add").size(13)].spacing(6).align_y(Alignment::Center))
        .padding([6, 10])
        .style(style::btn(Btn::Ghost))
        .on_press(Message::OpenAddProfile);
    let mut col = column![section("Your profiles", add.into())];
    if app.profiles.is_empty() {
        col = col.push(
            container(muted("No profiles yet.\nAdd a WireGuard .conf or OpenVPN .ovpn file from your VPN provider.").center())
                .padding([24, 8])
                .center_x(Length::Fill),
        );
    }
    for (name, kind, auth) in &app.profiles {
        if !matches(app, name) {
            continue;
        }
        let on = active.as_deref() == Some(name.as_str());
        let wg = kind == "wireguard";
        let sub = format!("{}{}", if wg { "WireGuard" } else { "OpenVPN · for the Raspberry Pi" }, if *auth { " · with login" } else { "" });
        let mut trailing: Vec<El> = Vec::new();
        if pi {
            trailing.push(icon_btn(icons::send(), Message::VpnToPi(name.clone()), "Copy to the Raspberry Pi"));
        }
        trailing.push(icon_btn(icons::trash(), Message::Ask(Pending::DeleteProfile(name.clone())), "Delete"));
        trailing.push(if wg {
            connect_btn(Message::VpnConnect(name.clone()), on)
        } else {
            container(text("PI ONLY").size(10).font(BOLD)).padding([2, 7]).style(style::pill(MUTED)).into()
        });
        col = col.push(list_row(
            container(svg(icons::lock()).width(18).height(18)).width(30).center_x(30).into(),
            name.clone(),
            sub,
            on,
            trailing,
        ));
    }
    col.push(muted("WireGuard profiles connect this computer (all of its traffic). OpenVPN profiles can be sent to the Raspberry Pi.").color(FAINT))
        .spacing(2)
        .into()
}

fn cloud_panel(app: &App) -> El<'_> {
    let c = &app.cloud;
    if c.is_null() {
        return column![section("Microsoft Azure", Space::new().into()), muted("Loading…")].into();
    }
    let signout: El = if c["account"].is_string() && c["deployment"].is_null() {
        button(t("Sign out").size(12)).padding([4, 10]).style(style::btn(Btn::Ghost)).on_press(Message::CloudLogout).into()
    } else {
        Space::new().into()
    };
    let mut col = column![section("Microsoft Azure", signout)].spacing(6);
    if c["account"].is_null() {
        let l = &c["login"];
        let mut card = column![
            text("Sign in to Azure").size(15).font(BOLD),
            muted("Use your Microsoft account. Geo Unlocker creates and deletes the server for you; nothing else in your account is touched."),
        ]
        .spacing(8);
        if l.is_null() || (l["done"].as_bool() == Some(true) && l["ok"].as_bool() != Some(true)) {
            if l["done"].as_bool() == Some(true) {
                card = card.push(text(s(l, "output")).size(12).color(BAD));
            }
            card = card.push(button(t("Sign in").font(SEMIBOLD)).padding([8, 16]).style(style::btn(Btn::Primary)).on_press(Message::CloudLogin));
        } else {
            let url = s(l, "url");
            card = card
                .push(row![t("1. Open "), button(text(url.trim_start_matches("https://").to_string()).size(14).color(style::ACCENT2)).padding(0).style(style::btn(Btn::Icon)).on_press(Message::OpenLink(url.clone())), t(" and sign in")].align_y(Alignment::Center))
                .push(t("2. Enter this code when asked:"))
                .push({
                    let code = s(l, "device_code");
                    let just_copied = app.copied.as_ref().is_some_and(|(c, at)| *c == code && at.elapsed().as_secs() < 3);
                    row![
                        text(code.clone()).size(26).font(BOLD),
                        space::horizontal(),
                        button(t(if just_copied { "✔ Copied" } else { "Copy" }).font(SEMIBOLD))
                            .padding([7, 14])
                            .style(style::btn(if just_copied { Btn::Ghost } else { Btn::Primary }))
                            .on_press(Message::Copy(code)),
                    ]
                    .align_y(Alignment::Center)
                })
                .push(muted("Waiting for you to finish signing in…"));
        }
        return col.push(container(card).padding(14).style(style::panel(CARD, 12.0, Some(LINE)))).into();
    }
    col = col.push(muted(format!("Signed in as {}", s(c, "account"))));
    let m = &app.status["mode"];
    if !c["deployment"].is_null() {
        let d = &c["deployment"];
        let active = m["kind"].as_str() == Some("cloud");
        let cc = s(d, "country");
        let created = d["created"].as_u64().unwrap_or(0);
        let age = geo_common::now().saturating_sub(created);
        let pi = app.status["pi"]["url"].is_string();
        let card = column![
            row![flag(&cc, 30.0), column![text(country_name(&cc)).size(15).font(BOLD), muted("Your Azure server")].width(Length::Fill), if active { connected_pill() } else { Space::new().into() }]
                .spacing(10)
                .align_y(Alignment::Center),
            row![muted("Region").width(90), t(s(d, "region")).size(13)],
            row![muted("Address").width(90), t(s(d, "ip")).size(13)],
            row![muted("Running for").width(90), t(fmt_dur(age)).size(13)],
            row![
                muted("Cost so far").width(90),
                t(format!(
                    "≈ US${:.2} (≈ US${:.3}/hour, plus data)",
                    age as f64 / 3600.0 * c["usd_per_hour"].as_f64().unwrap_or(0.0),
                    c["usd_per_hour"].as_f64().unwrap_or(0.0)
                ))
                .size(13)
            ],
            row![
                button(row![svg(icons::power()).width(14).height(14), t("Connect").size(13)].spacing(6)).padding([7, 10]).style(style::btn(Btn::Primary)).on_press(Message::CloudConnect),
                button(row![svg(icons::send()).width(14).height(14), t("Use on the Pi").size(13)].spacing(6)).padding([7, 10]).style(style::btn(Btn::Ghost)).on_press_maybe(pi.then_some(Message::CloudToPi)),
                button(row![svg(icons::trash()).width(14).height(14), t("Delete").size(13)].spacing(6)).padding([7, 10]).style(style::btn(Btn::Danger)).on_press(Message::Ask(Pending::Destroy)),
            ]
            .spacing(6),
        ]
        .spacing(8);
        col = col.push(container(card).padding(14).style(style::panel(CARD, 12.0, Some(LINE))));
        col = col.push(note(muted("It costs money while it exists. Delete it when you're done.").into(), false));
    } else {
        let subs: Vec<String> = c["options"]["subscriptions"].as_array().into_iter().flatten().map(|x| s(x, "name")).collect();
        if let Some(e) = c["options"]["error"].as_str() {
            col = col.push(note(text(e.to_string()).size(12).into(), true));
        }
        if subs.is_empty() {
            col = col.push(note(
                column![
                    t("No active Azure subscription.").font(SEMIBOLD),
                    muted("Create one (the free account works), then come back:"),
                    button(text("azure.microsoft.com/free").size(13).color(style::ACCENT2)).padding(0).style(style::btn(Btn::Icon)).on_press(Message::OpenLink("https://azure.microsoft.com/free/".into())),
                ]
                .spacing(4)
                .into(),
                true,
            ));
        } else if subs.len() > 1 {
            let selected = c["options"]["subscriptions"].as_array().into_iter().flatten().find(|x| Some(s(x, "id")) == app.cloud_sub).map(|x| s(x, "name"));
            col = col.push(muted("Subscription")).push(pick_list(subs, selected, Message::CloudSub).width(Length::Fill));
        }
        let mut codes: Vec<(String, String)> = c["regions"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect();
        codes.sort_by_key(|(k, _)| country_name(k));
        col = col.push(section("Create a server in", muted(codes.len().to_string()).into()));
        for (cc, region) in codes {
            let name = country_name(&cc);
            if !matches(app, &format!("{name} {cc} {region}")) {
                continue;
            }
            col = col.push(list_row(flag(&cc, 22.0), name, region, false, vec![connect_btn(Message::Ask(Pending::Deploy(cc.clone())), false)]));
        }
    }
    col.push(note(muted(format!("💲 {} Note: big streaming services block many cloud (data-centre) addresses, so a server may not unlock every service.", s(c, "cost_note"))).into(), false)).into()
}

// ---------------------------------------------------------------- map stage
fn stage(app: &App) -> El<'_> {
    let v = app.conn();
    let w = map::world();
    let home = app.home.as_ref().and_then(|h| h.at);
    let exit = if v.state == "on" {
        app.exit.as_ref().filter(|e| v.cc.is_empty() || e.country == v.cc).and_then(|e| e.at).or_else(|| w.points.get(&v.cc).copied())
    } else {
        None
    };
    let map_canvas = canvas(MapCanvas {
        view: app.view,
        size: app.map_size,
        land: &app.land,
        markers: app.markers(),
        selected: (!v.cc.is_empty()).then(|| v.cc.clone()),
        home,
        exit,
    })
    .width(Length::Fill)
    .height(Length::Fill);

    let (badge_text, badge_color, badge_sub) = match v.state {
        "on" => (
            "CONNECTED",
            OK,
            format!(
                "Exit: {}",
                app.exit.as_ref().filter(|e| !e.city.is_empty()).map(|e| format!("{}, {}", e.city, country_name(&e.country))).unwrap_or_else(|| country_name(&v.cc))
            ),
        ),
        "wait" => (
            "CONNECTING…",
            WARN,
            app.status["job"]["lines"].as_array().and_then(|l| l.last()).and_then(Value::as_str).unwrap_or("Setting up the connection…").to_string(),
        ),
        _ => ("UNPROTECTED", BAD, "Your traffic uses your normal connection".to_string()),
    };
    let badge = container(
        column![text(badge_text).size(16).font(BOLD).color(badge_color), text(badge_sub).size(12).color(MUTED)].align_x(Alignment::Center).spacing(2),
    )
    .padding(Padding { top: 10.0, right: 34.0, bottom: 14.0, left: 34.0 })
    .style(|_: &iced::Theme| container::Style {
        background: Some(SIDE.into()),
        border: iced::Border { color: LINE, width: 1.0, radius: iced::border::Radius { top_left: 0.0, top_right: 0.0, bottom_right: 26.0, bottom_left: 26.0 } },
        ..Default::default()
    });
    let legend_item = |c: Color, label: &'static str| {
        row![container(Space::new().width(9).height(9)).style(move |_: &iced::Theme| container::Style { background: Some(c.into()), border: iced::Border { radius: 5.0.into(), ..Default::default() }, ..Default::default() }), text(label).size(12).color(FAINT)]
            .spacing(6)
            .align_y(Alignment::Center)
    };
    let zoom_btn = |h: svg::Handle, m: Message| button(center(svg(h).width(16).height(16))).width(32).height(32).style(style::btn(Btn::Chip)).on_press(m);
    let overlay_top = row![
        column![legend_item(ACCENT, "You"), legend_item(OK, "Exit location"), legend_item(style::MARKER, "Available")].spacing(4).padding(16).width(Length::FillPortion(1)),
        container(badge).center_x(Length::FillPortion(2)),
        container(row![zoom_btn(icons::minus(), Message::MapZoom(1.0 / 1.5)), zoom_btn(icons::globe(), Message::MapReset), zoom_btn(icons::plus(), Message::MapZoom(1.5))].spacing(6))
            .padding(16)
            .align_right(Length::FillPortion(1)),
    ];
    let mut layers: Vec<El> = vec![map_canvas.into(), column![overlay_top].into()];
    if let Some(toast) = toast(app) {
        layers.push(container(toast).align_bottom(Length::Fill).padding(16).into());
    }
    column![stack(layers).width(Length::Fill).height(Length::Fill), traffic(app, &v)].width(Length::Fill).height(Length::Fill).into()
}

fn toast(app: &App) -> Option<El<'_>> {
    let (title, body, error, spinner): (String, String, bool, bool) = if let Some((msg, err, _)) = &app.flash {
        (if *err { "Something went wrong".into() } else { "Done".into() }, msg.clone(), *err, false)
    } else {
        let j = &app.status["job"];
        if !j.is_object() {
            return None;
        }
        let running = j["running"].as_bool() == Some(true);
        let err = j["error"].as_str().map(String::from);
        let id = format!("{}|{}", j["title"], j["error"]);
        if (!running && err.is_none()) || app.dismissed_job.as_deref() == Some(id.as_str()) {
            return None;
        }
        let lines: Vec<String> = j["lines"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
        let mut body = lines[lines.len().saturating_sub(6)..].join("\n");
        if let Some(e) = &err {
            body = format!("{body}\n{e}");
        }
        (format!("{}{}", s(j, "title"), if err.is_some() { " failed" } else if running { "…" } else { "" }), body, err.is_some(), running)
    };
    let _ = spinner;
    Some(
        container(
            column![
                row![text(title).size(14).font(SEMIBOLD), space::horizontal(), icon_btn(icons::close(), Message::DismissToast, "Dismiss")].align_y(Alignment::Center),
                text(body).size(12).color(if error { BAD } else { MUTED }),
            ]
            .spacing(4),
        )
        .max_width(480)
        .padding([10, 14])
        .style(style::floating)
        .into(),
    )
}

fn traffic<'a>(app: &'a App, v: &super::ConnView) -> El<'a> {
    let since = app.status["connected_at"].as_u64().unwrap_or(0);
    let session = if since > 0 && v.state != "off" { fmt_dur(geo_common::now().saturating_sub(since)) } else { "—".into() };
    let (dv, uv) = app.volume.map(|(d, u)| (fmt_bytes(d), fmt_bytes(u))).unwrap_or(("—".into(), "—".into()));
    let (ds, us) = app.speeds.map(|(d, u)| (fmt_rate(d), fmt_rate(u))).unwrap_or(("—".into(), "—".into()));
    let stat = |k: &'static str, val: String, c: Color| row![text(k).size(13).color(MUTED).width(110), text(val).size(13).color(c)];
    let stats = column![
        text("Session traffic").size(13).font(SEMIBOLD).color(MUTED),
        stat("Session", session, TEXT),
        stat("Down volume", dv, TEXT),
        stat("Up volume", uv, TEXT),
        stat("Down speed", ds, OK),
        stat("Up speed", us, BAD),
    ]
    .spacing(4)
    .width(250);
    let max_label = if app.speeds.is_some() { fmt_rate(map::chart_max(&app.samples)) } else { String::new() };
    let chart = column![
        row![space::horizontal(), text(max_label).size(11).color(FAINT)],
        canvas(map::Chart { samples: &app.samples }).width(Length::Fill).height(110),
        row![text("60 seconds").size(11).color(FAINT), space::horizontal(), text("now").size(11).color(FAINT)],
    ]
    .spacing(2)
    .width(Length::Fill);
    container(row![stats, chart].spacing(22)).padding([14, 22]).style(style::panel(style::BG, 0.0, Some(LINE))).into()
}

// ---------------------------------------------------------------- drawers & dialogs
fn add_profile(app: &App) -> El<'_> {
    let f = &app.form;
    column![
        text("Add a VPN profile").size(17).font(BOLD),
        muted("WireGuard .conf or OpenVPN .ovpn file from your VPN provider"),
        row![
            button(t("Choose file…")).padding([8, 12]).style(style::btn(Btn::Ghost)).on_press(Message::PickFile),
            text(if f.file_name.is_empty() { "No file chosen".to_string() } else { f.file_name.clone() }).size(13).color(MUTED),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        muted("Name"),
        text_input("e.g. US-NewYork", &f.name).on_input(Message::FormName).padding(9).style(style::input),
        muted("Username (OpenVPN, if your provider requires one)"),
        text_input("", &f.user).on_input(Message::FormUser).padding(9).style(style::input),
        muted("Password"),
        text_input("", &f.pass).on_input(Message::FormPass).secure(true).padding(9).style(style::input),
        row![
            space::horizontal(),
            button(t("Cancel")).padding([8, 14]).style(style::btn(Btn::Ghost)).on_press(Message::CloseOverlay),
            button(t("Save").font(SEMIBOLD)).padding([8, 14]).style(style::btn(Btn::Primary)).on_press(Message::SaveProfile),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .into()
}

fn pi_drawer(app: &App) -> El<'_> {
    let i = &app.pi_info;
    let mut col = column![
        muted("The Pi routes devices that can't run a VPN (smart TVs, monitors, streaming sticks). Send it profiles or your cloud server from here."),
        muted("Pi address"),
        text_input("192.168.1.10", &app.pi_url).on_input(Message::PiUrl).padding(9).style(style::input),
        muted("Pi password (leave empty to keep the saved one)"),
        text_input("", &app.pi_pass).on_input(Message::PiPass).secure(true).padding(9).style(style::input),
        button(t("Save & test").font(SEMIBOLD)).padding([8, 14]).style(style::btn(Btn::Primary)).on_press(Message::PiSave),
    ]
    .spacing(8);
    if i["checking"].as_bool() == Some(true) {
        col = col.push(muted("Checking…"));
    } else if let Some(e) = i["error"].as_str() {
        col = col.push(note(text(format!("✖ {e}")).size(13).color(BAD).into(), true));
    } else if i["status"].is_object() {
        let st = &i["status"];
        let devs = st["devices"].as_array().cloned().unwrap_or_default();
        let mut card = column![
            text("● Connected to the Pi").size(14).font(SEMIBOLD).color(OK),
            row![muted("Routing through").width(120), t(st["active"].as_str().unwrap_or("Nothing (normal internet)").to_string()).size(13)],
            row![muted("Devices").width(120), t(devs.len().to_string()).size(13)],
        ]
        .spacing(6);
        for d in devs {
            card = card.push(row![t(if s(&d, "name").is_empty() { "Device".into() } else { s(&d, "name") }).size(13), space::horizontal(), muted(s(&d, "ip"))]);
        }
        let url = s(i, "url");
        card = card.push(button(text(format!("Open the Pi's page ({url})")).size(12).color(style::ACCENT2)).padding(0).style(style::btn(Btn::Icon)).on_press(Message::OpenLink(url.clone())));
        col = col.push(container(card).padding(14).style(style::panel(CARD, 12.0, Some(LINE))));
    }
    col.into()
}

fn updates_drawer(app: &App) -> El<'_> {
    let u = &app.status["update"];
    let st = u["state"].as_str().unwrap_or("idle");
    let enabled = u["enabled"].as_bool() == Some(true);
    let checked = u["checked_at"].as_u64().unwrap_or(0);
    let last = if checked == 0 { "not yet".to_string() } else { format!("{} ago", fmt_dur(geo_common::now().saturating_sub(checked))) };
    let busy = matches!(st, "checking" | "downloading" | "installing");
    let mut col = column![container(
        column![
            row![muted("Version").width(110), t(s(u, "current")).size(13)],
            row![muted("Latest").width(110), t(if u["latest"].is_string() { s(u, "latest") } else { "—".into() }).size(13)],
            row![muted("Last check").width(110), t(last).size(13)],
        ]
        .spacing(4)
    )
    .padding(14)
    .style(style::panel(CARD, 12.0, Some(LINE)))]
    .spacing(10);
    let off = app.conn().state == "off";
    let state_el: El = match st {
        _ if !enabled => note(muted("This build doesn't update itself (it wasn't built by the release pipeline).").into(), false),
        "ready" => column![
            note(
                text(format!(
                    "Version {} is ready. {}",
                    s(u, "latest"),
                    if off { "It's installing now." } else { "It installs automatically when you disconnect, or install it now (this disconnects you)." }
                ))
                .size(13)
                .into(),
                true
            ),
            button(t("Install now & restart").font(SEMIBOLD)).padding([8, 14]).style(style::btn(Btn::Primary)).on_press(Message::Ask(Pending::InstallUpdate)),
        ]
        .spacing(8)
        .into(),
        "checking" => muted("Checking for updates…").into(),
        "downloading" => muted(format!("Downloading {}…", s(u, "latest"))).into(),
        "installing" => muted("Installing…").into(),
        "error" => note(text(format!("✖ {}", s(u, "error"))).size(13).color(BAD).into(), true),
        _ if u["latest"].is_string() => text("✔ You're up to date.").size(14).color(OK).into(),
        _ => Space::new().into(),
    };
    col = col.push(state_el);
    col = col
        .push(checkbox(u["auto"].as_bool() == Some(true)).label("Install updates automatically").on_toggle_maybe(enabled.then_some(Message::UpdAuto)))
        .push(muted("Updates come from the project's GitHub releases and are only installed if their signature checks out. They never interrupt a connection: they install when you disconnect."))
        .push(row![
            button(t("Check now")).padding([8, 14]).style(style::btn(Btn::Ghost)).on_press_maybe((enabled && !busy).then_some(Message::UpdCheck)),
            if u["notes_url"].is_string() {
                Element::from(button(t("Release notes")).padding([8, 14]).style(style::btn(Btn::Ghost)).on_press(Message::OpenLink(s(u, "notes_url"))))
            } else {
                Space::new().into()
            },
        ]
        .spacing(8))
        .push(muted("Third-party components: Tor, Wintun, boringtun, tun2proxy, iced, Natural Earth, flag-icons. See NOTICE.md in the install folder.").color(FAINT));
    let _ = (RAISE, Point::ORIGIN);
    col.into()
}

// ---------------------------------------------------------------- technical details
fn mono_block<'a>(app: &App, title: &str, subtitle: &str, body: String) -> El<'a> {
    let just_copied = app.copied.as_ref().is_some_and(|(c, at)| *c == body && at.elapsed().as_secs() < 3);
    let mut head = row![text(title.to_string()).size(14).font(BOLD), space::horizontal()].align_y(Alignment::Center);
    if !body.is_empty() {
        head = head.push(
            button(t(if just_copied { "✔ Copied" } else { "Copy" }).size(12))
                .padding([4, 10])
                .style(style::btn(Btn::Ghost))
                .on_press(Message::Copy(body.clone())),
        );
    }
    let mut col = column![head].spacing(6);
    if !subtitle.is_empty() {
        col = col.push(muted(subtitle.to_string()));
    }
    col = col.push(
        container(text(if body.is_empty() { "—".to_string() } else { body }).size(12).font(iced::Font::MONOSPACE))
            .width(Length::Fill)
            .padding([10, 12])
            .style(style::panel(style::BG, 9.0, Some(LINE))),
    );
    col.into()
}

fn json_pretty(v: &Value) -> String {
    if v.is_null() {
        String::new()
    } else {
        serde_json::to_string_pretty(v).unwrap_or_default()
    }
}

fn advanced_drawer(app: &App) -> El<'_> {
    let a = &app.advanced;
    if a.is_null() {
        return muted("Loading…").into();
    }
    if let Some(e) = a["error"].as_str() {
        return note(text(format!("✖ {e}")).size(13).color(BAD).into(), true);
    }
    // Connection summary
    let m = &a["mode"];
    let tun = &a["tunnel"];
    let tor = &a["tor"];
    let mut conn = vec![format!("mode:            {}", describe_mode(m))];
    match m["kind"].as_str() {
        Some("vpn") | Some("cloud") => {
            conn.push(format!("tunnel:          {}", if tun["kind"].is_null() { "not running".into() } else { format!("WireGuard (built in), profile {}", s(tun, "label")) }));
            conn.push(format!(
                "last handshake:  {}",
                tun["handshake_age"].as_u64().map(|x| format!("{x} s ago")).unwrap_or_else(|| "none yet".into())
            ));
            conn.push(format!("received / sent: {} / {}", fmt_bytes(tun["rx"].as_u64().unwrap_or(0)), fmt_bytes(tun["tx"].as_u64().unwrap_or(0))));
        }
        Some("tor") => {
            conn.push(format!("tor:             {} (bootstrap {}%)", if tor["running"].as_bool() == Some(true) { "running" } else { "stopped" }, tor["bootstrap"].as_u64().unwrap_or(0)));
            conn.push(format!("socks proxy:     {}", s(tor, "socks")));
            if m["system"].as_bool() != Some(false) {
                conn.push(format!("tor adapter:     {}", if a["tor_tunnel"].as_bool() == Some(true) { "up" } else { "down" }));
            }
        }
        _ => conn.push("tunnel:          none (normal connection)".into()),
    }
    conn.push(format!("data folder:     {}", s(a, "data_folder")));
    let changes = json_pretty(&a["network_changes"]);
    if !changes.is_empty() {
        conn.push(String::new());
        conn.push("network changes in effect (undone on disconnect):".into());
        conn.push(changes);
    }

    let cloud = &a["cloud"];
    let calls: Vec<String> = cloud["api_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let mut line = format!("{} {} {}\n   → {}", s(c, "time"), s(c, "method"), s(c, "url"), s(c, "status"));
            if !c["body"].is_null() {
                let body = json_pretty(&c["body"]);
                let lines: Vec<&str> = body.lines().collect();
                let shown = lines.iter().take(40).map(|l| format!("   {l}")).collect::<Vec<_>>().join("\n");
                line = format!("{line}\n{shown}{}", if lines.len() > 40 { "\n   …" } else { "" });
            }
            line
        })
        .collect();
    let server = &cloud["server"];
    let server_note = if server.is_null() {
        "Template: the values in <angle brackets> are filled in when you create a server.".to_string()
    } else {
        format!("Your server in {} ({}). Azure runs this once, at first boot (custom data). The server's private key is hidden.", s(server, "region"), s(server, "ip"))
    };
    let tor_out = a["tor_output"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
    let log = a["log"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>();
    let log = log[log.len().saturating_sub(150)..].join("\n");

    column![
        muted("For advanced users. Private keys and passwords are shown as <hidden>. Times are UTC. Updates every 2 seconds."),
        mono_block(app, "Connection", "", conn.join("\n")),
        mono_block(app, "WireGuard configuration", "The profile used by the current VPN or cloud connection.", s(a, "wireguard_config")),
        mono_block(app, "Cloud server setup script", &server_note, s(cloud, "setup_script")),
        mono_block(
            app,
            "Azure API calls",
            "Every request that created or deleted your server (Azure Resource Manager REST API), newest last. Status polling is omitted.",
            calls.join("\n\n")
        ),
        mono_block(app, "Deleting the server", "", s(cloud, "delete")),
        mono_block(app, "Tor output", "", tor_out),
        mono_block(app, "App log", "The last 150 lines; the full log is geo-unlocker.log in the data folder.", log),
    ]
    .spacing(16)
    .into()
}

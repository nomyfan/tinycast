//! A script's output in the popup, as it comes and once it is done: what a
//! script in `output` mode shows. Leaving the view stops the script.

use std::time::Duration;

use nsh_core::{element::ImageSrc, keymap::When, prelude::*};

use crate::{
    Launcher, back_button, icons, row_icon,
    scripts::{self, Outcome, ScriptIcon},
};

/// The lines kept, the last ones: a script printing for ever must not take
/// all the memory.
const KEPT_LINES: usize = 5000;
/// How long the lines gather before they show together: a line at a time,
/// a fast script (`seq 5000`) would lay the text out thousands of times.
const BATCH: Duration = Duration::from_millis(40);

#[derive(Clone, PartialEq)]
enum Status {
    Running,
    Done(Outcome),
}

pub(crate) fn script_view(launcher: Launcher) -> impl IntoElement {
    let script = launcher
        .script
        .get()
        .expect("Mode::Output comes with its script");
    let lines = signal(Vec::<String>::new());
    let status = signal(Status::Running);
    let output = element_ref();

    let env = launcher.settings.with_untracked(|s| s.env.clone());
    let mut run = scripts::start(&script, &env);
    // The view's task: when the view goes, the run is dropped with it, and
    // the script stopped.
    spawn(async move {
        while let Some(first) = run.line().await {
            let mut batch = vec![first];
            while let Some(line) = run.line_now() {
                batch.push(line);
            }
            lines.update(|all| {
                all.extend(batch);
                let dropped = all.len().saturating_sub(KEPT_LINES);
                all.drain(..dropped);
            });
            request_animation_frame(move |_| output.scroll_to_end(false));
            timer(BATCH).await;
        }
        status.set(Status::Done(run.outcome().await));
    });

    // No palette here to answer Escape (the popup's `close_on_escape` is
    // off): back to the launcher's list.
    let always = KeyOptions {
        when: When::Always,
        ..KeyOptions::default()
    };
    on_key_with("escape", always, move |_| launcher.back());

    view()
        .flex_1()
        .child(
            view()
                .h(px(48.0))
                .flex_row()
                .items_center()
                .gap_2p5()
                .px_4()
                .app_class("border-b border-foreground/10")
                .child(back_button(launcher))
                .child(script_icon(script.icon.clone()))
                .child(
                    text(script.name.clone())
                        .flex_1()
                        .app_class("text-base")
                        .number_of_lines(1),
                )
                .child(
                    text(move || status_label(&status.get())).app_class(move || {
                        let color = match status.get() {
                            Status::Done(Outcome::Failed(_)) => "text-destructive",
                            _ => "text-muted-foreground",
                        };
                        Some(format!("text-sm {color}").into())
                    }),
                ),
        )
        .child(
            scroll().element_ref(&output).flex_1().child(
                text(move || {
                    lines.with(|lines| match (lines.is_empty(), status.get()) {
                        (true, Status::Done(_)) => "No output".to_owned(),
                        _ => lines.join("\n"),
                    })
                })
                .selectable(true)
                .app_class("px-4 py-3 text-xs font-mono"),
            ),
        )
}

fn status_label(status: &Status) -> String {
    match status {
        Status::Running => "Running…".into(),
        Status::Done(Outcome::Succeeded) => "Done".into(),
        Status::Done(Outcome::Failed(why)) => why.clone(),
    }
}

/// A script's icon, the size of the launcher's.
pub(crate) fn script_icon(icon: ScriptIcon) -> AnyElement {
    match icon {
        ScriptIcon::Emoji(emoji) => view()
            .w(px(22.0))
            .h(px(22.0))
            .items_center()
            .justify_center()
            .child(text(emoji).app_class("text-base").number_of_lines(1))
            .into_element(),
        ScriptIcon::Image(src) => row_icon(move || ImageSrc::from(src.clone())).into_element(),
        ScriptIcon::Default => {
            row_icon(|| ImageSrc::from(icons::svg_uri(icons::SCRIPT_SVG))).into_element()
        }
    }
}

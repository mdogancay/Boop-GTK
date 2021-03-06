use crate::{
    command_pallete::CommandPalleteDialog,
    executor::{self},
    script::Script,
    scriptmap::ScriptMap,
    util::StringExt,
};
use eyre::{Context, Result};
use gdk_pixbuf::prelude::*;
use gladis::Gladis;
use glib::SourceId;
use gtk::{prelude::*, Label, Revealer, ShortcutsWindow};
use sourceview::prelude::*;

use executor::{ExecutorError, TextReplacement};
use gtk::{AboutDialog, ApplicationWindow, Button, ModelButton};
use std::{
    path::Path,
    sync::{Arc, RwLock},
};

pub const NOTIFICATION_LONG_DELAY: u32 = 5000;

#[derive(Gladis, Clone, Shrinkwrap)]
pub struct AppWidgets {
    #[shrinkwrap(main_field)]
    window: ApplicationWindow,

    header_button: Button,
    source_view: sourceview::View,
    // status_bar: Statusbar,
    notification_revealer: Revealer,
    notification_label: Label,
    notification_button: Button,

    re_execute_last_script_button: ModelButton,
    reset_scripts_button: ModelButton,
    config_directory_button: ModelButton,
    more_scripts_button: ModelButton,
    about_button: ModelButton,
    shortcuts_button: ModelButton,

    about_dialog: AboutDialog,
}

#[derive(Clone, Shrinkwrap)]
pub struct App {
    #[shrinkwrap(main_field)]
    widgets: AppWidgets,
    scripts: Arc<RwLock<ScriptMap>>,
    notification_source_id: Arc<RwLock<Option<SourceId>>>,
    last_script_executed: Arc<RwLock<Option<String>>>,
}

impl App {
    pub(crate) fn new(config_dir: &Path, scripts: Arc<RwLock<ScriptMap>>) -> Result<Self> {
        let widgets = AppWidgets::from_resource("/fyi/zoey/Boop-GTK/boop-gtk.glade")
            .wrap_err("Failed to load boop-gtk.glade")?;

        let app = App {
            widgets,
            scripts,
            notification_source_id: Arc::new(RwLock::new(None)),
            last_script_executed: Arc::new(RwLock::new(None)),
        };

        app.widgets
            .about_dialog
            .set_version(Some(env!("CARGO_PKG_VERSION")));

        for (_, script) in app
            .scripts
            .read()
            .expect("Scripts lock is poisoned")
            .0
            .iter()
        {
            if let Some(author) = &script.metadata.author {
                app.about_dialog
                    .add_credit_section(&format!("{} script", &script.metadata.name), &[author]);
            }
        }

        app.setup_syntax_highlighting(config_dir)?;

        // close notification on dismiss
        {
            let notification_revealer = app.notification_revealer.clone();
            app.notification_button
                .connect_button_press_event(move |_button, _event| {
                    notification_revealer.set_reveal_child(false);
                    Inhibit(false)
                });
        }

        // re-execute last script
        {
            let app_ = app.clone();
            app.re_execute_last_script_button
                .connect_clicked(move |_| app_.re_execute().expect("Failed to re-execute script"));
        }

        // reset the state of each script
        {
            let scripts = app.scripts.clone();
            app.reset_scripts_button.connect_clicked(move |_| {
                for (_, script) in scripts
                    .write()
                    .expect("Scripts lock is poisoned")
                    .0
                    .iter_mut()
                {
                    script.kill_thread();
                }
            });
        }

        // launch config directory in default file manager
        {
            let config_dir_str = config_dir.display().to_string();
            let app_ = app.clone();
            app.config_directory_button.connect_clicked(move |_| {
                if let Err(open_err) = open::that(config_dir_str.clone()) {
                    error!("could not launch config directory: {}", open_err);
                    app_.post_notification_error(
                        "Failed to launch config directory",
                        NOTIFICATION_LONG_DELAY,
                    );
                }
            });
        }

        // launch more scripts page in default web browser
        {
            let app_ = app.clone();
            app.more_scripts_button.connect_clicked(move |_| {
                if let Err(open_err) = open::that("https://boop.okat.best/scripts/") {
                    error!("could not launch website: {}", open_err);
                    app_.post_notification_error(
                        "Failed to launch website",
                        NOTIFICATION_LONG_DELAY,
                    );
                }
            });
        }

        {
            let about_dialog: AboutDialog = app.about_dialog.clone();
            app.about_button.connect_clicked(move |_| {
                let responce = about_dialog.run();
                if responce == gtk::ResponseType::DeleteEvent
                    || responce == gtk::ResponseType::Cancel
                {
                    about_dialog.hide();
                }
            });
        }

        {
            let app_ = app.clone();
            app.shortcuts_button
                .connect_clicked(move |_| app_.open_shortcuts_window());
        }

        {
            let app_ = app.clone();
            app.header_button.connect_clicked(move |_| {
                app_.run_command_pallete()
                    .expect("Failed to run command pallete")
            });
        }

        Ok(app)
    }

    fn new_shortcuts_window(window: &gtk::ApplicationWindow) -> ShortcutsWindow {
        let shortcut_window = gtk::ShortcutsWindowBuilder::new()
            .transient_for(window)
            .build();

        let section = gtk::ShortcutsSectionBuilder::new().visible(true).build();

        {
            let group = gtk::ShortcutsGroupBuilder::new()
                .title("Test")
                .visible(true)
                .build();

            group.add(
                &gtk::ShortcutsShortcutBuilder::new()
                    .action_name("app.command_pallete")
                    .visible(true)
                    .build(),
            );
            group.add(
                &gtk::ShortcutsShortcutBuilder::new()
                    .action_name("app.re_execute_script")
                    .visible(true)
                    .build(),
            );
            group.add(
                &gtk::ShortcutsShortcutBuilder::new()
                    .action_name("app.quit")
                    .visible(true)
                    .build(),
            );
        }

        // genral group
        {
            let group = gtk::ShortcutsGroupBuilder::new()
                .title("General")
                .visible(true)
                .build();

            let shortcuts = [
                ("Open Command Pallette", "<Primary><Shift>P"),
                ("Quit", "<Primary>Q"),
            ];

            for (title, accelerator) in &shortcuts {
                group.add(
                    &gtk::ShortcutsShortcutBuilder::new()
                        .title(title)
                        .accelerator(accelerator)
                        .visible(true)
                        .build(),
                );
            }

            section.add(&group);
        }

        // editor group
        {
            let group = gtk::ShortcutsGroupBuilder::new()
                .title("Editor")
                .visible(true)
                .build();

            let shortcuts = [
                ("Undo", "<Primary>Z"),
                ("Redo", "<Primary><Shift>Z"),
                ("Move line up", "<Alt>Up"),
                ("Move line down", "<Alt>Down"),
                ("Move cursor backwards one word", "<Primary>Left"),
                ("Move cursor forward one word", "<Primary>Right"),
                ("Move cursor to beginning of previous line", "<Primary>Up"),
                ("Move cursor to end of next line", "<Primary>Down"),
                ("Move cursor to beginning of line", "<Primary>Page_Up"),
                ("Move cursor to end of line", "<Primary>Page_Down"),
                ("Move cursor to beginning of document", "<Primary>Home"),
                ("Move cursor to end of document", "<Primary>End"),
            ];

            for (title, accelerator) in &shortcuts {
                group.add(
                    &gtk::ShortcutsShortcutBuilder::new()
                        .title(title)
                        .accelerator(accelerator)
                        .visible(true)
                        .build(),
                );
            }

            section.add(&group);
        }

        shortcut_window.add(&section);

        shortcut_window
    }

    fn post_notification(&self, text: &str, delay: u32) {
        let notification_source_id = self.notification_source_id.clone();
        let notification_revealer = self.notification_revealer.clone();
        let notification_label = self.notification_label.clone();

        {
            notification_label.set_markup(text);
            notification_revealer.set_reveal_child(true);

            let mut source_id = notification_source_id.write().unwrap();

            // cancel old notification timeout
            if source_id.is_some() {
                glib::source_remove(source_id.take().unwrap());
            }

            // dismiss after 3000ms
            let new_source_id = {
                let notification_source_id = notification_source_id.clone();
                glib::timeout_add_local(delay, move || {
                    notification_revealer.set_reveal_child(false);
                    *notification_source_id.write().unwrap() = None;
                    Continue(false)
                })
            };

            source_id.replace(new_source_id);
        }
    }

    pub fn post_notification_error(&self, text: &str, delay: u32) {
        self.post_notification(
            &format!(
                r#"<span foreground="red" weight="bold">ERROR:</span> {}"#,
                text
            ),
            delay,
        );
    }

    fn setup_syntax_highlighting(&self, config_dir: &Path) -> Result<()> {
        let language_manager = sourceview::LanguageManager::get_default()
            .ok_or(eyre!("Failed to get language manager"))?;

        // add config_dir to language manager's search path
        let dirs = language_manager.get_search_path();
        let mut dirs: Vec<&str> = dirs.iter().map(|s| s.as_ref()).collect();
        let config_dir_path = config_dir.to_string_lossy().to_string();
        dirs.push(&config_dir_path);
        language_manager.set_search_path(&dirs);

        info!("language manager search directorys: {}", dirs.join(":"));

        let boop_language = language_manager.get_language("boop");
        if boop_language.is_none() {
            self.post_notification(
                r#"<span foreground="red">ERROR:</span> failed to load language file"#,
                NOTIFICATION_LONG_DELAY,
            );
        }

        // set language
        let buffer: sourceview::Buffer = self
            .source_view
            .get_buffer()
            .ok_or(eyre!("Failed to get buffer"))?
            .downcast::<sourceview::Buffer>()
            .map_err(|_| eyre!("Failed to downcast TextBuffer to sourceview Buffer"))?;
        buffer.set_highlight_syntax(true);
        buffer.set_language(boop_language.as_ref());

        Ok(())
    }

    // fn push_error_(status_bar: gtk::Statusbar, context_id: u32, error: impl std::fmt::Display) {
    //     status_bar.push(context_id, &format!("ERROR: {}", error));
    // }

    // pub fn push_error(&self, error: impl std::fmt::Display) {
    //     App::push_error_(self.status_bar.clone(), self.context_id, error);
    // }

    pub fn open_shortcuts_window(&self) {
        let window = self.window.clone();
        let shortcuts_window = App::new_shortcuts_window(&window);
        shortcuts_window.show_all();
    }

    pub fn run_command_pallete(&self) -> Result<()> {
        let dialog = CommandPalleteDialog::new(&self.window, self.scripts.clone())?;
        dialog.show_all();

        if let gtk::ResponseType::Accept = dialog.run() {
            let selected: &str = dialog
                .get_selected()
                .ok_or(eyre!("Command pallete dialog didn't return a selection"))?;

            *self.last_script_executed.write().unwrap() = Some(String::from(selected));
            self.execute_script(selected)?;
        }

        dialog.close();

        Ok(())
    }

    pub fn re_execute(&self) -> Result<()> {
        if let Some(script_key) = &*self.last_script_executed.read().unwrap() {
            self.execute_script(&script_key)
                .wrap_err("Failed to execute script")
        } else {
            warn!("no last script");
            Ok(())
        }
    }

    fn execute_script(&self, script_key: &str) -> Result<()> {
        let mut script_map = self.scripts.write().expect("Scripts lock is poisoned");
        let script: &mut Script = script_map
            .0
            .get_mut(script_key)
            .ok_or(eyre!("Script not in map"))?;

        info!("executing {}", script.metadata.name);

        let buffer = &self
            .source_view
            .get_buffer()
            .ok_or(eyre!("Failed to get buffer"))?;

        let buffer_text = buffer
            .get_text(&buffer.get_start_iter(), &buffer.get_end_iter(), false)
            .ok_or(eyre!("Failed to get buffer text"))?;

        let selection_text = buffer
            .get_selection_bounds()
            .map(|(start, end)| buffer.get_text(&start, &end, false))
            .flatten()
            .map(|s| s.to_string());

        let status_result = script.execute(buffer_text.as_str(), selection_text.as_deref());

        match status_result {
            Ok(status) => {
                // TODO: how to handle multiple messages?
                if let Some(error) = status.error() {
                    self.post_notification(
                        &format!(
                            r#"<span foreground="red" weight="bold">ERROR:</span> {}"#,
                            error
                        ),
                        NOTIFICATION_LONG_DELAY,
                    );
                } else if let Some(info) = status.info() {
                    self.post_notification(&info, NOTIFICATION_LONG_DELAY);
                }
                self.do_replacement(status.clone().into_replacement())
                    .wrap_err_with(|| format!("Failed to make replacement: {:?}", status))?;
            }
            Err(err) => {
                let executor_err = err.downcast::<ExecutorError>().unwrap(); // can't recover from other errors

                error!("Exception: {:?}", executor_err);
                self.post_notification_error(
                    &executor_err.to_notification_string(),
                    NOTIFICATION_LONG_DELAY,
                );
            }
        }

        Ok(())
    }

    fn do_replacement(&self, replacement: TextReplacement) -> Result<()> {
        let buffer = &self
            .source_view
            .get_buffer()
            .ok_or(eyre!("Failed to get buffer"))?;

        match replacement {
            TextReplacement::Full(text) => {
                info!("replacing full text");

                let safe_text = text
                    .remove_null_bytes()
                    .wrap_err("Failed to remove null bytes from text")?;

                buffer.set_text(&safe_text);
            }
            TextReplacement::Selection(text) => {
                info!("replacing selection");

                let safe_text = text
                    .remove_null_bytes()
                    .wrap_err("Failed to remove null bytes from text")?;

                match &mut buffer.get_selection_bounds() {
                    Some((start, end)) => {
                        buffer.delete(start, end);
                        buffer.insert(start, &safe_text);
                    }
                    None => {
                        error!("tried to do a selection replacement, but no text is selected!");
                    }
                }
            }
            TextReplacement::Insert(insertions) => {
                let insert_text = insertions.join("");
                info!("inserting {} bytes", insert_text.len());

                let safe_text = insert_text
                    .remove_null_bytes()
                    .wrap_err("Failed to remove null bytes from text")?;

                match &mut buffer.get_selection_bounds() {
                    Some((start, end)) => {
                        buffer.delete(start, end);
                        buffer.insert(start, &safe_text);
                    }
                    None => {
                        let mut insert_point =
                            buffer.get_iter_at_offset(buffer.get_property_cursor_position());
                        buffer.insert(&mut insert_point, &safe_text)
                    }
                }
            }
            TextReplacement::None => {
                info!("no text to replace");
            }
        }

        self.source_view.grab_focus();

        Ok(())
    }
}

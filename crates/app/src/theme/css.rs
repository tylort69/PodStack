//! The app stylesheet: Material 3 color roles as `@define-color`s plus component styles.
//!
//! Style classes widgets use (see also `widgets/`):
//! - type scale: `display-small`, `headline-{large,medium,small}`, `title-{large,medium,small}`,
//!   `body-{large,medium,small}`, `label-{large,medium,small}`
//! - text colors: `dim` (on_surface_variant), `primary-text`, `secondary-text`, `tertiary-text`, `error-text`,
//!   `success-text`, `warning-text`
//! - surfaces: `card` (elevated), `card-filled`, `card-outlined`, `surface-container-{low,high,highest}`
//! - buttons: `filled`, `tonal`, `outlined`, `text-button`, `elevated`, `icon-button` (+ `filled`/`tonal`), `fab`,
//!   `danger`
//! - `chip` (+ `selected`), `badge`, `status-{good,warn,bad,info,neutral}` (container-colored pills)
//! - `icon` (Material Icons ligature font; text is the icon name), `avatar`, `stat-tile`, `m3-table`, `snackbar`

use super::scheme::Scheme;

pub fn stylesheet(s: &Scheme) -> String {
    let mut css = String::with_capacity(32 * 1024);
    for (k, v) in &s.colors {
        css.push_str(&format!("@define-color {k} {v};\n"));
    }
    let (success, on_success, success_c, on_success_c, warning, warning_c, on_warning_c) = if s.dark {
        ("#7fd88f", "#00391a", "#1f5130", "#9bf5aa", "#ffb951", "#5c4200", "#ffdea8")
    } else {
        ("#1e6b35", "#ffffff", "#b4f1bd", "#00210c", "#7a5900", "#ffdea8", "#261900")
    };
    css.push_str(&format!(
        "@define-color success {success};\n@define-color on_success {on_success};\n@define-color success_container {success_c};\n\
         @define-color on_success_container {on_success_c};\n@define-color warning {warning};\n@define-color warning_container {warning_c};\n\
         @define-color on_warning_container {on_warning_c};\n"
    ));
    // GTK's own named colors, so stock widgets follow the scheme too
    css.push_str(
        "@define-color accent_color @primary;\n@define-color accent_bg_color @primary;\n@define-color accent_fg_color @on_primary;\n\
         @define-color window_bg_color @surface;\n@define-color window_fg_color @on_surface;\n@define-color view_bg_color @surface;\n\
         @define-color view_fg_color @on_surface;\n@define-color theme_bg_color @surface;\n@define-color theme_fg_color @on_surface;\n\
         @define-color theme_base_color @surface;\n@define-color theme_text_color @on_surface;\n@define-color theme_selected_bg_color @primary;\n\
         @define-color theme_selected_fg_color @on_primary;\n@define-color borders @outline_variant;\n",
    );
    css.push_str(STATIC);
    css
}

const STATIC: &str = r#"
* { outline-color: alpha(@primary, 0.6); }

window, window.background {
  background-color: @surface;
  color: @on_surface;
  font-family: "Inter", "Roboto", "Noto Sans", sans-serif;
  font-size: 14px;
}

label { color: inherit; }
label.dim, .dim { color: @on_surface_variant; }
.primary-text { color: @primary; }
.secondary-text { color: @secondary; }
.tertiary-text { color: @tertiary; }
.error-text { color: @error; }
.success-text { color: @success; }
.warning-text { color: @warning; }
selection { background-color: alpha(@primary, 0.35); color: @on_surface; }

/* ---------- type scale ---------- */
.display-small   { font-size: 36px; font-weight: 400; letter-spacing: 0; }
.headline-large  { font-size: 32px; font-weight: 400; }
.headline-medium { font-size: 28px; font-weight: 400; }
.headline-small  { font-size: 24px; font-weight: 400; }
.title-large     { font-size: 22px; font-weight: 400; }
.title-medium    { font-size: 16px; font-weight: 500; letter-spacing: 0.15px; }
.title-small     { font-size: 14px; font-weight: 500; letter-spacing: 0.1px; }
.body-large      { font-size: 16px; font-weight: 400; letter-spacing: 0.5px; }
.body-medium     { font-size: 14px; font-weight: 400; letter-spacing: 0.25px; }
.body-small      { font-size: 12px; font-weight: 400; letter-spacing: 0.4px; }
.label-large     { font-size: 14px; font-weight: 500; letter-spacing: 0.1px; }
.label-medium    { font-size: 12px; font-weight: 500; letter-spacing: 0.5px; }
.label-small     { font-size: 11px; font-weight: 500; letter-spacing: 0.5px; }
.numeric { font-feature-settings: "tnum"; }

/* ---------- icons (Material Icons ligatures) ---------- */
.icon { font-family: "Material Icons"; font-size: 24px; font-weight: 400; font-feature-settings: "liga"; min-width: 24px; }
.icon.icon-18 { font-size: 18px; min-width: 18px; }
.icon.icon-20 { font-size: 20px; min-width: 20px; }
.icon.icon-36 { font-size: 36px; min-width: 36px; }
.icon.icon-48 { font-size: 48px; min-width: 48px; }

/* ---------- surfaces ---------- */
.surface-container-lowest { background-color: @surface_container_lowest; }
.surface-container-low { background-color: @surface_container_low; }
.surface-container { background-color: @surface_container; }
.surface-container-high { background-color: @surface_container_high; }
.surface-container-highest { background-color: @surface_container_highest; }

.card {
  background-color: @surface_container_low;
  border-radius: 16px;
  padding: 16px;
  box-shadow: 0 1px 2px alpha(@shadow, 0.30), 0 1px 3px 1px alpha(@shadow, 0.15);
}
.card-filled { background-color: @surface_container_highest; border-radius: 16px; padding: 16px; }
.card-outlined { background-color: @surface; border: 1px solid @outline_variant; border-radius: 16px; padding: 16px; }
.card.flush, .card-filled.flush, .card-outlined.flush { padding: 0; }
.card-header { padding-bottom: 8px; }

/* ---------- buttons ---------- */
button {
  border-radius: 999px;
  min-height: 40px;
  padding: 0 24px;
  font-weight: 500;
  font-size: 14px;
  letter-spacing: 0.1px;
  border: none;
  box-shadow: none;
  background-image: none;
  background-color: transparent;
  color: @primary;
  transition: background-color 150ms ease, box-shadow 150ms ease, color 150ms ease;
}
button:hover { background-color: alpha(@primary, 0.08); }
button:active { background-color: alpha(@primary, 0.12); }
button:disabled { color: alpha(@on_surface, 0.38); background-color: transparent; }
button label { color: inherit; }

button.filled { background-color: @primary; color: @on_primary; }
button.filled:hover { background-color: mix(@primary, @on_primary, 0.08); box-shadow: 0 1px 2px alpha(@shadow, 0.3), 0 1px 3px 1px alpha(@shadow, 0.15); }
button.filled:active { background-color: mix(@primary, @on_primary, 0.12); }
button.filled:disabled, button.tonal:disabled, button.elevated:disabled { background-color: alpha(@on_surface, 0.12); color: alpha(@on_surface, 0.38); box-shadow: none; }

button.tonal { background-color: @secondary_container; color: @on_secondary_container; }
button.tonal:hover { background-color: mix(@secondary_container, @on_secondary_container, 0.08); }
button.tonal:active { background-color: mix(@secondary_container, @on_secondary_container, 0.12); }

button.outlined { border: 1px solid @outline; color: @primary; }
button.outlined:disabled { border-color: alpha(@on_surface, 0.12); }

button.text-button { padding: 0 12px; }

button.elevated { background-color: @surface_container_low; color: @primary; box-shadow: 0 1px 2px alpha(@shadow, 0.3), 0 1px 3px 1px alpha(@shadow, 0.15); }

button.danger.filled { background-color: @error; color: @on_error; }
button.danger:not(.filled) { color: @error; }
button.danger:not(.filled):hover { background-color: alpha(@error, 0.08); }

button.icon-button { min-width: 40px; min-height: 40px; padding: 0; color: @on_surface_variant; }
button.icon-button:hover { background-color: alpha(@on_surface_variant, 0.08); }
button.icon-button.filled { background-color: @primary; color: @on_primary; }
button.icon-button.tonal { background-color: @secondary_container; color: @on_secondary_container; }
button.icon-button:checked { color: @primary; background-color: alpha(@primary, 0.12); }

button.fab {
  min-width: 56px; min-height: 56px; border-radius: 16px; padding: 0 16px;
  background-color: @primary_container; color: @on_primary_container;
  box-shadow: 0 1px 3px alpha(@shadow, 0.3), 0 4px 8px 3px alpha(@shadow, 0.15);
}
button.fab:hover { background-color: mix(@primary_container, @on_primary_container, 0.08); }

button.flat, button.image-button { padding: 0 8px; }

/* toggle buttons used as segmented buttons */
.segmented { border: 1px solid @outline; border-radius: 999px; }
.segmented > button { border-radius: 0; min-height: 38px; padding: 0 16px; color: @on_surface; border-right: 1px solid @outline; }
.segmented > button:first-child { border-radius: 999px 0 0 999px; }
.segmented > button:last-child { border-radius: 0 999px 999px 0; border-right: none; }
.segmented > button:checked { background-color: @secondary_container; color: @on_secondary_container; }

/* ---------- chips & badges ---------- */
.chip, button.chip {
  border: 1px solid @outline_variant; border-radius: 8px; min-height: 32px; padding: 0 12px;
  color: @on_surface_variant; font-size: 14px; font-weight: 500; background-color: transparent;
}
button.chip:hover { background-color: alpha(@on_surface_variant, 0.08); }
.chip.selected, button.chip:checked { background-color: @secondary_container; color: @on_secondary_container; border-color: @secondary_container; }
.badge {
  background-color: @error; color: @on_error; border-radius: 999px; min-width: 16px; min-height: 16px;
  padding: 0 4px; font-size: 11px; font-weight: 500;
}
.badge.small { min-width: 6px; min-height: 6px; padding: 0; }
.status-good, .status-warn, .status-bad, .status-info, .status-neutral {
  border-radius: 8px; padding: 2px 8px; font-size: 12px; font-weight: 500;
}
.status-good { background-color: @success_container; color: @on_success_container; }
.status-warn { background-color: @warning_container; color: @on_warning_container; }
.status-bad { background-color: @error_container; color: @on_error_container; }
.status-info { background-color: @primary_container; color: @on_primary_container; }
.status-neutral { background-color: @surface_container_highest; color: @on_surface_variant; }

/* ---------- inputs ---------- */
entry, spinbutton, searchentry, passwordentry, dropdown > button, combobox > button {
  background-color: @surface_container_highest;
  color: @on_surface;
  border-radius: 4px 4px 0 0;
  border: none;
  border-bottom: 1px solid @on_surface_variant;
  min-height: 48px;
  padding: 0 12px;
  box-shadow: none;
  background-image: none;
}
entry:focus-within, spinbutton:focus-within, searchentry:focus-within { border-bottom: 2px solid @primary; }
entry text, spinbutton text, searchentry text { color: @on_surface; }
entry text placeholder, searchentry text placeholder { color: @on_surface_variant; }
searchentry { border-radius: 999px; border-bottom: none; min-height: 48px; padding: 0 16px; background-color: @surface_container_high; }
searchentry:focus-within { border-bottom: none; box-shadow: inset 0 0 0 2px alpha(@primary, 0.6); }
dropdown > button, combobox > button { color: @on_surface; font-weight: 400; padding: 0 12px; }
dropdown > button:hover { background-color: mix(@surface_container_highest, @on_surface, 0.08); }
spinbutton > button { min-height: 32px; min-width: 32px; padding: 0; border-radius: 999px; color: @on_surface_variant; }
textview, textview text { background-color: @surface_container_highest; color: @on_surface; }
textview { border-radius: 4px; padding: 8px; }

switch { background-color: @surface_container_highest; border: 2px solid @outline; border-radius: 999px; min-width: 48px; min-height: 28px; }
switch slider { background-color: @outline; border-radius: 999px; min-width: 16px; min-height: 16px; margin: 4px; box-shadow: none; border: none; }
scale slider, switch slider, scrollbar slider { outline: none; }
switch:checked { background-color: @primary; border-color: @primary; }
switch:checked slider { background-color: @on_primary; min-width: 24px; min-height: 24px; margin: 0; }

checkbutton check, check { border: 2px solid @on_surface_variant; border-radius: 2px; background-color: transparent; min-width: 14px; min-height: 14px; }
checkbutton check:checked, check:checked { background-color: @primary; border-color: @primary; color: @on_primary; }
checkbutton radio, radio { border: 2px solid @on_surface_variant; border-radius: 999px; background-color: transparent; }
checkbutton radio:checked, radio:checked { border-color: @primary; color: @primary; }

scale trough { background-color: @secondary_container; min-height: 4px; border-radius: 999px; }
scale highlight { background-color: @primary; border-radius: 999px; }
scale slider { background-color: @primary; min-width: 20px; min-height: 20px; border-radius: 999px; box-shadow: none; border: none; }

progressbar trough { background-color: @surface_container_highest; min-height: 4px; border-radius: 999px; }
progressbar progress { background-color: @primary; min-height: 4px; border-radius: 999px; }
progressbar.thick trough, progressbar.thick progress { min-height: 8px; }
progressbar.tertiary progress { background-color: @tertiary; }
progressbar.success progress { background-color: @success; }
progressbar.warning progress { background-color: @warning; }
progressbar.error progress { background-color: @error; }
levelbar block.filled { background-color: @primary; }
levelbar block.empty { background-color: @surface_container_highest; }

/* ---------- popovers, menus, tooltips, dialogs ---------- */
popover > contents, popover.menu > contents {
  background-color: @surface_container; color: @on_surface; border-radius: 12px; padding: 8px 0;
  box-shadow: 0 2px 6px 2px alpha(@shadow, 0.15), 0 1px 2px alpha(@shadow, 0.3); border: none;
}
popover > arrow { background-color: @surface_container; border: none; }
popover modelbutton { min-height: 48px; padding: 0 12px; border-radius: 0; color: @on_surface; }
popover modelbutton:hover { background-color: alpha(@on_surface, 0.08); }
popover.padded > contents { padding: 16px; }
tooltip, tooltip.background { background-color: @inverse_surface; color: @inverse_on_surface; border-radius: 4px; padding: 4px 8px; box-shadow: none; border: none; }
tooltip label { color: @inverse_on_surface; font-size: 12px; }

/* Hyprland rounds and borders the dialog window itself */
window.m3-dialog { background-color: @surface_container_high; }
window.m3-dialog > .dialog-surface { padding: 24px; }
.dialog-title { font-size: 24px; font-weight: 400; color: @on_surface; }
.dialog-actions { padding-top: 16px; }
.scrim { background-color: alpha(@scrim, 0.32); }

/* ---------- navigation drawer ---------- */
.nav-drawer { background-color: @surface_container_low; padding: 12px; min-width: 280px; }
.nav-status { background-color: @surface_container_low; padding: 10px 28px 16px 28px; border-top: 1px solid @outline_variant; }
.nav-drawer .app-title { padding: 12px 16px 16px 16px; }
.nav-section-label { color: @on_surface_variant; font-size: 14px; font-weight: 500; padding: 18px 16px 8px 16px; }
button.nav-item {
  min-height: 56px; border-radius: 999px; padding: 0 16px 0 16px; color: @on_surface_variant; font-weight: 500;
}
button.nav-item.compact { min-height: 48px; }
button.nav-item:hover { background-color: alpha(@on_surface, 0.08); color: @on_surface; }
button.nav-item.active { background-color: @secondary_container; color: @on_secondary_container; }
button.nav-item.active .icon { color: @on_secondary_container; }
button.nav-item .badge { background-color: @primary; color: @on_primary; }
.nav-divider { background-color: @outline_variant; min-height: 1px; margin: 8px 16px; }

/* navigation rail (narrow windows) */
.nav-rail { background-color: @surface_container_low; padding: 12px 0; min-width: 80px; }
button.rail-item { min-width: 56px; min-height: 32px; padding: 0; border-radius: 999px; color: @on_surface_variant; }
button.rail-item.active { background-color: @secondary_container; color: @on_secondary_container; }
.rail-label { font-size: 12px; font-weight: 500; color: @on_surface_variant; }

/* ---------- top app bar & tabs ---------- */
.top-app-bar { background-color: @surface; padding: 8px 16px 8px 24px; min-height: 64px; }
.top-app-bar.scrolled { background-color: @surface_container; }
.top-app-bar .title { font-size: 22px; font-weight: 400; color: @on_surface; }

.m3-tabs { border-bottom: 1px solid @surface_variant; padding: 0 16px; }
button.m3-tab {
  border-radius: 0; min-height: 48px; padding: 0 16px; color: @on_surface_variant; font-weight: 500;
  border-bottom: 3px solid transparent;
}
button.m3-tab:hover { background-color: alpha(@on_surface, 0.08); color: @on_surface; }
button.m3-tab:checked, button.m3-tab.active { color: @primary; border-bottom-color: @primary; }

/* ---------- lists & tables ---------- */
list, listview, .m3-list { background-color: transparent; color: @on_surface; }
listview > row, list > row { padding: 0; border-radius: 0; }
listview > row:hover, list > row:hover { background-color: alpha(@on_surface, 0.08); }
listview > row:selected, list > row:selected { background-color: @secondary_container; color: @on_secondary_container; }
.list-item { min-height: 56px; padding: 8px 16px; }
.list-item.three-line { min-height: 88px; }
.list-item .headline { font-size: 16px; color: @on_surface; }
.list-item .supporting { font-size: 14px; color: @on_surface_variant; }

columnview, columnview.m3-table { background-color: transparent; color: @on_surface; }
columnview > header > button {
  background-color: @surface_container; color: @on_surface_variant; border-radius: 0; min-height: 44px;
  padding: 0 12px; font-weight: 500; font-size: 13px; border: none; border-bottom: 1px solid @outline_variant;
}
columnview > header > button:hover { background-color: mix(@surface_container, @on_surface, 0.08); color: @on_surface; }
columnview > header > button sort-indicator { color: @primary; }
columnview > listview > row { border-bottom: 1px solid alpha(@outline_variant, 0.5); }
columnview > listview > row > cell { padding: 10px 12px; }
columnview > listview > row:selected { background-color: alpha(@secondary_container, 0.7); color: @on_secondary_container; }
.table-frame { border-radius: 12px; border: 1px solid @outline_variant; background-color: @surface_container_lowest; }

/* ---------- misc ---------- */
separator { background-color: @outline_variant; min-height: 1px; min-width: 1px; }
scrollbar { background-color: transparent; }
scrollbar slider { background-color: alpha(@on_surface_variant, 0.4); border-radius: 999px; min-width: 6px; min-height: 6px; border: none; margin: 0; }
scrollbar slider:hover { background-color: alpha(@on_surface_variant, 0.7); }
.avatar { border-radius: 999px; }
picture.rounded, image.rounded { border-radius: 12px; }

.stat-tile { background-color: @surface_container; border-radius: 16px; padding: 16px 20px; min-width: 150px; }
.stat-tile .stat-label { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; }
.stat-tile .stat-value { color: @on_surface; font-size: 24px; font-weight: 400; }
.stat-tile .stat-sub { color: @on_surface_variant; font-size: 12px; }
.stat-tile.accent { background-color: @primary_container; }
.stat-tile.accent .stat-label, .stat-tile.accent .stat-value, .stat-tile.accent .stat-sub { color: @on_primary_container; }

.banner { background-color: @tertiary_container; color: @on_tertiary_container; border-radius: 12px; padding: 12px 16px; }
.banner.error { background-color: @error_container; color: @on_error_container; }
.banner.info { background-color: @primary_container; color: @on_primary_container; }

.snackbar {
  background-color: @inverse_surface; color: @inverse_on_surface; border-radius: 4px; padding: 4px 8px 4px 16px;
  min-height: 48px; box-shadow: 0 2px 6px 2px alpha(@shadow, 0.15), 0 1px 2px alpha(@shadow, 0.3);
}
.snackbar label { color: @inverse_on_surface; }
.snackbar button { color: @inverse_primary; }

.empty-state { padding: 48px; }
.empty-state .icon { color: @on_surface_variant; }

.character-header { background-color: @surface_container_low; border-radius: 28px; padding: 20px 24px; }
.skill-pip { min-width: 10px; min-height: 10px; border-radius: 2px; background-color: @surface_container_highest; border: 1px solid @outline_variant; }
.skill-pip.trained { background-color: @primary; border-color: @primary; }
.skill-pip.partial { background-color: alpha(@primary, 0.45); border-color: @primary; }
.skill-pip.queued { background-color: @tertiary_container; border-color: @tertiary; }

.color-swatch { border-radius: 999px; min-width: 40px; min-height: 40px; border: 2px solid transparent; padding: 0; }
.color-swatch.selected { border-color: @on_surface; }
"#;

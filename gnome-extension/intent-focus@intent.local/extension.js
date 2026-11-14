// Intent focus reporter for GNOME Shell 46-49 (Wayland and X11).
//
// Publishes the focused window on the *session* bus so the local `intentd`
// daemon can record app focus on Wayland. Nothing leaves the machine: there is
// no network code, no logging of titles, and the extension is inactive on the
// lock screen (default session modes).
//
// D-Bus API (name org.intent.Focus, path /org/intent/Focus):
//   GetFocus()   -> (s window_id, s app_id, s wm_class, s title, u pid)
//   GetVersion() -> (u version)
//   signal FocusChanged(s window_id, s app_id, s wm_class, s title, u pid)
// window_id is "" when no window has focus (overview, desktop).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.intent.Focus';
const OBJECT_PATH = '/org/intent/Focus';
const API_VERSION = 1;

const INTERFACE_XML = `
<node>
  <interface name="org.intent.Focus">
    <method name="GetFocus">
      <arg type="s" direction="out" name="window_id"/>
      <arg type="s" direction="out" name="app_id"/>
      <arg type="s" direction="out" name="wm_class"/>
      <arg type="s" direction="out" name="title"/>
      <arg type="u" direction="out" name="pid"/>
    </method>
    <method name="GetVersion">
      <arg type="u" direction="out" name="version"/>
    </method>
    <signal name="FocusChanged">
      <arg type="s" name="window_id"/>
      <arg type="s" name="app_id"/>
      <arg type="s" name="wm_class"/>
      <arg type="s" name="title"/>
      <arg type="u" name="pid"/>
    </signal>
  </interface>
</node>`;

const EMPTY = ['', '', '', '', 0];

export default class IntentFocusExtension extends Extension {
    enable() {
        this._window = null;
        this._windowSignals = [];
        this._lastKey = null;

        this._dbus = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);

        this._focusSignal = global.display.connect('notify::focus-window',
            () => this._onFocusChanged());
        this._onFocusChanged();
    }

    disable() {
        if (this._focusSignal) {
            global.display.disconnect(this._focusSignal);
            this._focusSignal = 0;
        }
        this._untrackWindow();
        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
        this._lastKey = null;
    }

    // --- D-Bus methods -----------------------------------------------------

    GetFocus() {
        return this._describe(this._window);
    }

    GetVersion() {
        return API_VERSION;
    }

    // --- internals ---------------------------------------------------------

    _describe(win) {
        if (!win)
            return EMPTY;
        let appId = '';
        try {
            const app = Shell.WindowTracker.get_default().get_window_app(win);
            appId = app?.get_id() ?? '';
        } catch {
            appId = '';
        }
        const pid = win.get_pid();
        return [
            String(win.get_id()),
            appId,
            win.get_wm_class() ?? '',
            win.get_title() ?? '',
            pid > 0 ? pid : 0,
        ];
    }

    _onFocusChanged() {
        const win = global.display.focus_window;
        if (win !== this._window) {
            this._untrackWindow();
            this._trackWindow(win);
        }
        this._emit();
    }

    _trackWindow(win) {
        this._window = win ?? null;
        if (!this._window)
            return;
        this._windowSignals = [
            this._window.connect('notify::title', () => this._emit()),
            this._window.connect('notify::wm-class', () => this._emit()),
            this._window.connect('unmanaged', () => {
                this._untrackWindow();
                this._emit();
            }),
        ];
    }

    _untrackWindow() {
        if (this._window) {
            for (const id of this._windowSignals)
                this._window.disconnect(id);
        }
        this._window = null;
        this._windowSignals = [];
    }

    _emit() {
        if (!this._dbus)
            return;
        const info = this._describe(this._window);
        const key = info.join('\u0000');
        if (key === this._lastKey)
            return;
        this._lastKey = key;
        this._dbus.emit_signal('FocusChanged', new GLib.Variant('(ssssu)', info));
    }
}

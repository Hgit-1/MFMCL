"""Exercise native dialogs in the installed v0.1.2 candidate, without IPC mocks."""
import os
import platform
import sqlite3
import subprocess
import time
from pathlib import Path

ROOT = Path.cwd()
FIXTURE = ROOT / 'fixture' / 'Release Picker Smoke Test.mflp'
NAME = 'Release Picker Smoke Test'
PLUGIN = 'mfmcl-release-picker-smoke'
CONTRIBUTION = '-Dmfmcl.releasePickerSmoke=true'
ACCEPT = ('I have read and agree', '我已阅读并同意')
SKIP = ('Skip', '跳过引导')
PLUGINS = ('Launcher plugins', '启动器插件')
IMPORT = ('Import a plugin package', '导入插件包')
APPROVE = ('Approve this version', '批准此版本')
CLOSE = ('Close for now', '先关闭')
REVIEW = ('Review plugin', '审核插件')
ENABLE = ('Enable ' + NAME, '启用 ' + NAME)
WINDOWS = platform.system() == 'Windows'


def wait(find, label, timeout=30):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        result = find()
        if result:
            return result
        time.sleep(0.35)
    raise RuntimeError('Timed out waiting for ' + label)


if WINDOWS:
    import pywinauto
    from pywinauto.keyboard import send_keys
    from pywinauto.uia_defines import NoPatternInterfaceError
    from pywinauto.controls.hwndwrapper import InvalidWindowHandle

    DATA = Path(os.environ['APPDATA']) / 'dev.mfmcl.launcher'
    desktop = pywinauto.Desktop(backend='uia')

    def win_find(proc, names, types=None, partial=False):
        for window in desktop.windows(process=proc.pid):
            try:
                for control in window.descendants():
                    name = control.element_info.name or ''
                    if types and control.element_info.control_type not in types:
                        continue
                    if any(expected in name if partial else name == expected for expected in names) and control.is_visible():
                        return control
            except (InvalidWindowHandle, RuntimeError, OSError):
                continue
        return None

    def app_find(proc, names, role='control'):
        types = ('Button', 'CheckBox', 'ToggleButton', 'Custom') if role == 'control' else None
        return win_find(proc, names, types, partial=role == 'text')

    def click(element):
        try:
            element.invoke()
        except (AttributeError, NoPatternInterfaceError):
            element.click_input()

    def file_panel(proc):
        for window in desktop.windows(process=proc.pid, class_name='#32770'):
            if window.is_visible() and window.element_info.name in IMPORT:
                return window
        return None

    def dismiss_panel(panel):
        panel.set_focus()
        send_keys('{ESC}')

    def fill_panel(panel):
        field = panel.child_window(control_type='Edit', auto_id='1148')
        if field.exists(timeout=2):
            field.set_edit_text(str(FIXTURE))
        else:
            panel.set_focus()
            send_keys('%n')
            send_keys(str(FIXTURE), with_spaces=True, vk_packet=True)
        button = panel.child_window(control_type='Button', auto_id='1')
        if button.exists(timeout=2):
            click(button.wrapper_object())
        else:
            send_keys('{ENTER}')

    def evidence(proc):
        for i, win in enumerate(desktop.windows(process=proc.pid)):
            win.capture_as_image().save(ROOT / 'evidence' / f'windows-{i}.png')
            print('Window:', win.element_info.name, win.element_info.class_name, flush=True)

else:
    import Quartz as Q
    import AppKit

    DATA = Path.home() / 'Library' / 'Application Support' / 'dev.mfmcl.launcher'

    def attr(element, key):
        status, result = Q.AXUIElementCopyAttributeValue(element, key, None)
        return result if status == 0 else None

    def elements(root):
        pending = [root]
        visited = set()
        while pending and len(visited) < 2500:
            node = pending.pop()
            if node in visited:
                continue
            visited.add(node)
            yield node
            pending.extend(reversed(attr(node, 'AXChildren') or []))

    def labels(node):
        return [label.strip() for key in ('AXTitle', 'AXDescription', 'AXValue', 'AXHelp')
                if isinstance(label := attr(node, key), str) and label.strip()]

    def app_find(proc, names, role='control'):
        for node in elements(Q.AXUIElementCreateApplication(proc.pid)):
            if role == 'control' and attr(node, 'AXRole') not in ('AXButton', 'AXCheckBox', 'AXSwitch', 'AXToggle', 'AXPopUpButton'):
                continue
            if any(expected in label if role == 'text' else expected == label for label in labels(node) for expected in names):
                return node
        return None

    def click(element):
        status = Q.AXUIElementPerformAction(element, 'AXPress')
        if status != 0:
            raise RuntimeError(f'AXPress failed ({status}) on {labels(element)}')

    def file_panel(proc):
        root = Q.AXUIElementCreateApplication(proc.pid)
        for node in elements(root):
            role = attr(node, 'AXRole')
            if role not in ('AXSheet', 'AXWindow', 'AXDialog'):
                continue
            if not any(label in IMPORT for label in labels(node)) and role == 'AXWindow':
                continue
            controls = {label for descendant in elements(node) for label in labels(descendant)}
            if 'Open' in controls and 'Cancel' in controls:
                return node
        return None

    def find_named(root, role, name):
        return next((node for node in elements(root) if attr(node, 'AXRole') == role and name in labels(node)), None)

    def key(code, flags=0, text=None):
        for down in (True, False):
            event = Q.CGEventCreateKeyboardEvent(None, code, down)
            Q.CGEventSetFlags(event, flags)
            if text:
                Q.CGEventKeyboardSetUnicodeString(event, len(text), text)
            Q.CGEventPost(Q.kCGHIDEventTap, event)

    def dismiss_panel(panel):
        click(wait(lambda: find_named(panel, 'AXButton', 'Cancel'), 'native Cancel'))

    def fill_panel(panel):
        assert Q.CGPreflightPostEventAccess(), 'Runner does not grant permission for native keyboard events'
        key(5, Q.kCGEventFlagMaskCommand | Q.kCGEventFlagMaskShift)
        time.sleep(0.5)
        key(0, text=str(FIXTURE))
        time.sleep(0.7)
        key(36)
        time.sleep(0.8)
        opening = wait(lambda: find_named(panel, 'AXButton', 'Open'), 'native Open')
        click(opening)

    def evidence(proc):
        subprocess.run(['/usr/sbin/screencapture', '-x', str(ROOT / 'evidence' / 'macos.png')], check=False)
        if Q.AXIsProcessTrusted():
            for node in list(elements(Q.AXUIElementCreateApplication(proc.pid)))[:300]:
                fields = labels(node)
                if fields:
                    print(attr(node, 'AXRole'), fields, flush=True)


def counts():
    db = DATA / 'mfmcl.sqlite'
    assert db.is_file(), db
    with sqlite3.connect(db) as conn:
        return conn.execute('select count(*) from plugin_approvals').fetchone()[0]


def start():
    return subprocess.Popen([os.environ['MFMCL_BIN']])


def stop(proc):
    if proc.poll() is None:
        if not WINDOWS:
            app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(proc.pid)
            if app:
                app.terminate()
        if WINDOWS:
            try:
                windows = desktop.windows(process=proc.pid, title='MFMCL')
                if windows:
                    windows[0].close()
            except Exception:
                pass
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.terminate()
            proc.wait(10)


def open_plugins(proc):
    click(wait(lambda: app_find(proc, PLUGINS), 'Plugins'))
    return wait(lambda: app_find(proc, IMPORT), 'Import plugin package')


def main():
    print(platform.platform(), platform.machine(), os.environ['ASSET'], os.environ['EXPECTED_SHA256'], flush=True)
    assert not (DATA / 'mfmcl.sqlite').exists(), 'Test needs a fresh runner profile'
    if not WINDOWS:
        assert Q.AXIsProcessTrusted(), 'Runner has not granted Accessibility permission to the test process'
    proc = start()
    try:
        click(wait(lambda: app_find(proc, ACCEPT), 'terms prompt'))
        click(wait(lambda: app_find(proc, SKIP), 'skip setup'))
        click(open_plugins(proc))
        panel = wait(lambda: file_panel(proc), 'native file picker')
        dismiss_panel(panel)
        wait(lambda: not file_panel(proc), 'native picker to close')
        assert not (DATA / 'plugins' / PLUGIN).exists()
        assert counts() == 0
        print('PASS: native picker opened; cancellation installed nothing', flush=True)

        click(wait(lambda: app_find(proc, IMPORT), 'Import plugin package'))
        fill_panel(wait(lambda: file_panel(proc), 'native file picker reopened'))
        wait(lambda: app_find(proc, APPROVE), 'approval confirmation')
        wait(lambda: app_find(proc, (CONTRIBUTION,), 'text'), 'exact JVM property in approval dialog')
        assert (DATA / 'plugins' / PLUGIN / 'manifest.json').is_file()
        assert counts() == 0
        click(wait(lambda: app_find(proc, CLOSE), 'dismiss approval'))
        wait(lambda: app_find(proc, REVIEW), 'review after dismissal')
        stop(proc)
        proc = start()
        open_plugins(proc)
        wait(lambda: app_find(proc, REVIEW), 'review after restart')
        assert counts() == 0
        print('PASS: path with spaces imported; dismissed approval stayed disabled after restart', flush=True)

        click(wait(lambda: app_find(proc, REVIEW), 'review pending plugin'))
        wait(lambda: app_find(proc, (CONTRIBUTION,), 'text'), 'exact contribution on review')
        click(wait(lambda: app_find(proc, APPROVE), 'explicit approval'))
        wait(lambda: counts() == 1, 'approval in native database')
        wait(lambda: app_find(proc, ENABLE), 'approved plugin switch')
        stop(proc)
        proc = start()
        open_plugins(proc)
        wait(lambda: app_find(proc, ENABLE), 'approved switch after restart')
        assert not app_find(proc, REVIEW) and counts() == 1
        print('PASS: explicit approval persisted after restart', flush=True)
    except Exception:
        try:
            evidence(proc)
        except Exception as error:
            print('Could not capture evidence:', error, flush=True)
        raise
    finally:
        stop(proc)


if __name__ == '__main__':
    main()

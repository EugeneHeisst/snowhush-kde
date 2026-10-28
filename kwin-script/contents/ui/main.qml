import QtQuick
import org.kde.kwin

// Wayland keeps window positions and the pointer from other clients, so KWin hands them to Snowhush over D-Bus.
Item {
    id: root

    property string lastWorld: ""
    property real lastX: -1
    property real lastY: -1
    property int quietTicks: 0

    DBusCall {
        id: worldCall
        service: "org.snowhush.Kde"
        path: "/Snowhush"
        dbusInterface: "org.snowhush.Kde"
        method: "Update"
    }

    DBusCall {
        id: cursorCall
        service: "org.snowhush.Kde"
        path: "/Snowhush"
        dbusInterface: "org.snowhush.Kde"
        method: "Cursor"
    }

    function onCurrent(w) {
        if (!w.onAllDesktops) {
            let here = false;
            for (const d of w.desktops) {
                if (d === Workspace.currentDesktop) {
                    here = true;
                    break;
                }
            }
            if (!here)
                return false;
        }
        const activities = w.activities;
        return activities.length === 0 || activities.indexOf(Workspace.currentActivity) >= 0;
    }

    function world() {
        const screens = [];
        for (const output of Workspace.screens) {
            const a = Workspace.clientArea(KWin.MaximizeArea, output, Workspace.currentDesktop);
            const g = output.geometry;
            screens.push({ name: output.name, area: [a.x, a.y, a.width, a.height], geometry: [g.x, g.y, g.width, g.height] });
        }
        const windows = [];
        let moving = false;
        const order = Workspace.stackingOrder;
        // Front to back.
        for (let i = order.length - 1; i >= 0; i--) {
            const w = order[i];
            if (!w || w.deleted || !w.managed || !(w.normalWindow || w.dialog || w.utility) || w.resourceClass === "snowhush-kde")
                continue;
            if (w.move || w.resize)
                moving = true;
            // 0: on screen, 1: on another desktop or activity, 2: minimized or hidden.
            let state = 0;
            if (w.minimized || w.hidden || w.opacity < 0.3)
                state = 2;
            else if (!onCurrent(w))
                state = 1;
            const f = w.frameGeometry;
            windows.push({
                id: String(w.internalId),
                x: Math.round(f.x), y: Math.round(f.y), w: Math.round(f.width), h: Math.round(f.height),
                output: w.output ? w.output.name : "",
                state: state,
                full: w.fullScreen,
                square: w.maximizedHorizontally || w.maximizedVertically || w.noBorder
            });
        }
        return JSON.stringify({ screens: screens, windows: windows, moving: moving });
    }

    Timer {
        interval: 8
        running: true
        repeat: true
        onTriggered: {
            const c = Workspace.cursorPos;
            if (c.x !== root.lastX || c.y !== root.lastY) {
                root.lastX = c.x;
                root.lastY = c.y;
                cursorCall.arguments = [c.x, c.y];
                cursorCall.call();
            }
            // Windows every other tick: 60 a second is plenty for a drag.
            root.quietTicks += 1;
            if (root.quietTicks < 2)
                return;
            root.quietTicks = 0;
            const json = root.world();
            if (json !== root.lastWorld) {
                root.lastWorld = json;
                worldCall.arguments = [json];
                worldCall.call();
            }
        }
    }

    // Resent now and then so Snowhush, started after KWin, catches up without waiting for a change.
    Timer {
        interval: 2000
        running: true
        repeat: true
        onTriggered: root.lastWorld = ""
    }
}

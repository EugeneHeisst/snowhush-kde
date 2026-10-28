import QtQuick
import org.kde.kwin

// Wayland keeps window positions and the pointer from other clients, so KWin hands them to Snowhush over D-Bus.
// Everything here waits on KWin's own change signals: with nothing moving, the script does no work at all.
Item {
    id: root

    property string lastWorld: ""
    property bool worldDirty: true
    property bool cursorDirty: false
    // Whether Snowhush answered the last call; while it isn't running only a rare probe goes out.
    property bool alive: false

    DBusCall {
        id: worldCall
        service: "org.snowhush.Kde"
        path: "/Snowhush"
        dbusInterface: "org.snowhush.Kde"
        method: "Update"
        onFinished: root.alive = true
        onFailed: {
            root.alive = false;
            root.lastWorld = "";
        }
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

    function relevant(w) {
        return w && !w.deleted && w.managed && (w.normalWindow || w.dialog || w.utility) && w.resourceClass !== "snowhush-kde";
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
            if (!relevant(w))
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

    function markWorld() {
        root.worldDirty = true;
        if (root.alive && !flush.running)
            flush.start();
    }

    function watch(w) {
        if (!w)
            return;
        w.frameGeometryChanged.connect(markWorld);
        w.minimizedChanged.connect(markWorld);
        w.hiddenChanged.connect(markWorld);
        w.opacityChanged.connect(markWorld);
        w.fullScreenChanged.connect(markWorld);
        w.desktopsChanged.connect(markWorld);
        w.activitiesChanged.connect(markWorld);
        w.interactiveMoveResizeStarted.connect(markWorld);
        w.interactiveMoveResizeFinished.connect(markWorld);
    }

    Connections {
        target: Workspace
        function onCursorPosChanged() {
            root.cursorDirty = true;
            if (root.alive && !flush.running)
                flush.start();
        }
        function onWindowAdded(w) {
            root.watch(w);
            root.markWorld();
        }
        function onWindowRemoved(w) {
            root.markWorld();
        }
        function onStackingOrderChanged() {
            root.markWorld();
        }
        function onCurrentDesktopChanged() {
            root.markWorld();
        }
        function onCurrentActivityChanged() {
            root.markWorld();
        }
        function onScreensChanged() {
            root.markWorld();
        }
    }

    Component.onCompleted: {
        for (const w of Workspace.stackingOrder)
            watch(w);
    }

    // Gathers a burst of changes into one call: at most about 120 a second while something moves.
    Timer {
        id: flush
        interval: 8
        repeat: false
        onTriggered: {
            if (root.cursorDirty) {
                root.cursorDirty = false;
                const c = Workspace.cursorPos;
                cursorCall.arguments = [c.x, c.y];
                cursorCall.call();
            }
            if (root.worldDirty) {
                root.worldDirty = false;
                const json = root.world();
                if (json !== root.lastWorld) {
                    root.lastWorld = json;
                    worldCall.arguments = [json];
                    worldCall.call();
                }
            }
        }
    }

    // Finds Snowhush when it starts after KWin, and repeats the window list now and then in case a call was lost.
    Timer {
        interval: 3000
        running: true
        repeat: true
        onTriggered: {
            root.lastWorld = "";
            root.worldDirty = true;
            if (!flush.running)
                flush.start();
        }
    }
}

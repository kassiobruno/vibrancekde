// Script do KWin carregado pelo daemon vibrance-kde.
// Avisa o daemon (via D-Bus) toda vez que a janela em foco muda, no mesmo
// espírito do SetWinEventHook(EVENT_SYSTEM_FOREGROUND) do vibranceGUI.
// Não faz polling: só reage a sinais do KWin.

const VK_SERVICE = "io.github.vibrance_kde";
const VK_PATH = "/io/github/vibrance_kde";
const VK_IFACE = "io.github.vibrance_kde.Daemon";

function describe(w) {
    if (!w) {
        return {};
    }
    var out = "";
    try {
        if (w.output && w.output.name) {
            out = String(w.output.name);
        }
    } catch (e) {}
    return {
        pid: Number(w.pid) || 0,
        "class": String(w.resourceClass || ""),
        name: String(w.resourceName || ""),
        caption: String(w.caption || ""),
        output: out,
        fullscreen: !!w.fullScreen
    };
}

function sendActive(w) {
    try {
        callDBus(VK_SERVICE, VK_PATH, VK_IFACE, "WindowActivated", JSON.stringify(describe(w)));
    } catch (e) {
        print("vibrance-kde: " + e);
    }
}

function sendList() {
    try {
        var list = [];
        var all = workspace.windowList();
        for (var i = 0; i < all.length; i++) {
            var w = all[i];
            if (w.normalWindow && w.pid > 0) {
                list.push(describe(w));
            }
        }
        callDBus(VK_SERVICE, VK_PATH, VK_IFACE, "WindowList", JSON.stringify(list));
    } catch (e) {
        print("vibrance-kde: " + e);
    }
}

workspace.windowActivated.connect(sendActive);
workspace.windowAdded.connect(sendList);
workspace.windowRemoved.connect(sendList);

sendActive(workspace.activeWindow);
sendList();

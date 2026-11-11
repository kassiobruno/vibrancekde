#!/usr/bin/env python3
"""Teste de ponta a ponta do daemon, sem KDE.

Usa um barramento D-Bus privado, um KWin falso (examples/fake_kwin.rs) e um
kscreen-doctor falso com estado (tests/fake-kscreen-doctor). O daemon real é
executado e recebe eventos de foco por D-Bus exatamente como o script do KWin
os enviaria.

Uso:  cargo build --examples && cargo build && \
      dbus-run-session -- python3 tests/e2e.py
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "debug", "vibrance-kde")
KWIN = os.path.join(ROOT, "target", "debug", "examples", "fake_kwin")
FAKE = os.path.join(ROOT, "tests", "fake-kscreen-doctor")

TMP = "/tmp/vk-e2e"
HOME = os.path.join(TMP, "home")
FAKE_DIR = os.path.join(TMP, "fake")
BINDIR = os.path.join(TMP, "bin")

SVC = "io.github.vibrance_kde"
PATH = "/io/github/vibrance_kde"
IFACE = "io.github.vibrance_kde.Daemon"

failures = []


def check(cond, msg):
    print(("  ok   " if cond else "  FAIL ") + msg)
    if not cond:
        failures.append(msg)


def state():
    with open(os.path.join(FAKE_DIR, "state.json")) as f:
        return {o["name"]: o for o in json.load(f)["outputs"]}


def wait_for(cond, timeout=10.0, step=0.1):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if cond():
                return True
        except Exception:
            pass
        time.sleep(step)
    return False


def gdbus(method, *args):
    cmd = ["gdbus", "call", "--session", "--dest", SVC, "--object-path", PATH,
           "--method", f"{IFACE}.{method}", *args]
    return subprocess.run(cmd, capture_output=True, text=True, timeout=15)


def window(cls, output="DP-1", caption=""):
    payload = json.dumps({"pid": 0, "class": cls, "name": cls, "caption": caption,
                          "output": output, "fullscreen": True})
    gdbus("WindowActivated", "'" + payload + "'")


def is_ours(o):
    """Perfil do vibrance-kde ATIVO. O KWin mantém o caminho antigo depois de voltar
    para sRGB, então só conta se a fonte de cor ainda for ICC."""
    return os.path.basename(o["iccProfilePath"]).startswith("vibrance-kde-") and o["source"] == "ICC"


def data_dir():
    """Onde o crate `dirs` guarda dados: macOS usa Library/Application Support, Linux usa XDG."""
    if sys.platform == "darwin":
        return os.path.join(HOME, "Library", "Application Support")
    return os.path.join(HOME, ".local", "share")


def config_dir():
    if sys.platform == "darwin":
        return os.path.join(HOME, "Library", "Application Support")
    return os.path.join(HOME, ".config")


def config_path():
    return os.path.join(config_dir(), "vibrance-kde", "config.toml")


def write_config(text):
    os.makedirs(os.path.dirname(config_path()), exist_ok=True)
    with open(config_path(), "w") as f:
        f.write(text)


CONFIG = """
enabled = true
windows_level = 50
affect_primary_only = false
never_switch_resolution = false

[[apps]]
name = "Counter-Strike 2"
process = "cs2"
level = 80

[[apps]]
name = "Jogo em janela"
process = "Windowed.exe"
level = 100
change_resolution = true
resolution = { width = 1920, height = 1080, refresh = 144 }
"""


def main():
    shutil.rmtree(TMP, ignore_errors=True)
    os.makedirs(BINDIR)
    os.makedirs(FAKE_DIR)
    shutil.copy(FAKE, os.path.join(BINDIR, "kscreen-doctor"))
    os.chmod(os.path.join(BINDIR, "kscreen-doctor"), 0o755)
    write_config(CONFIG)

    env = dict(os.environ, HOME=HOME, FAKE_KSCREEN_DIR=FAKE_DIR,
               PATH=BINDIR + os.pathsep + os.environ["PATH"], RUST_LOG="debug")
    for var in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "APPIMAGE"):
        env.pop(var, None)

    kwin = subprocess.Popen([KWIN], env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    time.sleep(0.7)
    log = open(os.path.join(TMP, "daemon.log"), "w")
    daemon = subprocess.Popen([BIN, "--minimized"], env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        print("== inicialização")
        check(wait_for(lambda: gdbus("Status").returncode == 0, 10), "serviço D-Bus do daemon responde")
        check(wait_for(lambda: daemon.poll() is None, 1), "daemon continua vivo mesmo sem bandeja (sem StatusNotifierWatcher)")
        st = json.loads(gdbus("Status").stdout.strip().strip("()',\n").replace("\\'", "'") or "{}") if False else None

        def kwin_connected():
            out = gdbus("Status").stdout
            return '"kwin_connected":true' in out.replace(" ", "")
        check(wait_for(kwin_connected, 10), "script de foco carregado no KWin (status.kwin_connected)")
        check(os.path.exists(os.path.join(data_dir(), "vibrance-kde", "kwin-focus.js")),
              "script kwin-focus.js instalado no diretório de dados")

        print("== jogo cadastrado ganha foco")
        window("cs2")
        check(wait_for(lambda: is_ours(state()["DP-1"]), 10), "DP-1 recebe perfil ICC do vibrance-kde")
        o = state()["DP-1"]
        check(o["source"] == "ICC", "colorProfileSource = ICC")
        check(o["power"] == "prefer accuracy", "colorPowerTradeoff = preferAccuracy (necessário p/ shader ICC do KWin)")
        check(o["iccProfilePath"].endswith("-80.icc"), "perfil do nível 80% (vibrance-kde-DP-1-80.icc)")
        check(not is_ours(state()["HDMI-A-1"]), "HDMI-A-1 não é alterado (jogo está em DP-1)")
        with open(o["iccProfilePath"], "rb") as f:
            head = f.read(200)
        check(head[36:40] == b"acsp" and head[12:16] == b"mntr", "perfil gravado é um ICC válido (acsp/mntr)")

        print("== outra janela ganha foco (volta ao nível do desktop)")
        window("firefox")
        check(wait_for(lambda: not is_ours(state()["DP-1"]), 10), "perfil removido de DP-1")
        o = state()["DP-1"]
        check(o["source"] == "sRGB" and o["power"].startswith("prefer efficiency"),
              "estado original restaurado (sRGB + eficiência)")
        leftover = [f for f in os.listdir(os.path.join(data_dir(), "color", "icc"))
                    if f.startswith("vibrance-kde-")]
        check(leftover == [], "perfis temporários apagados")

        print("== jogo no segundo monitor")
        window("CS2", output="HDMI-A-1")  # maiúsculas: comparação sem diferenciar caixa
        check(wait_for(lambda: is_ours(state()["HDMI-A-1"]), 10), "HDMI-A-1 recebe o perfil")
        check(not is_ours(state()["DP-1"]), "DP-1 permanece normal")
        window("firefox")
        check(wait_for(lambda: not is_ours(state()["HDMI-A-1"]), 10), "HDMI-A-1 restaurado")

        print("== troca de resolução (jogo em janela)")
        window("windowed", output="DP-1")  # casa com 'Windowed.exe' (ignora .exe e caixa)
        check(wait_for(lambda: state()["DP-1"]["currentModeId"] == "2", 10), "resolução trocada para 1920x1080@144")
        check(state()["DP-1"]["iccProfilePath"].endswith("-100.icc"), "nível 100% aplicado junto")
        window("firefox")
        check(wait_for(lambda: state()["DP-1"]["currentModeId"] == "1", 10), "resolução original restaurada")
        check(wait_for(lambda: not is_ours(state()["DP-1"]), 10), "cor restaurada")

        print("== opção 'nunca trocar a resolução'")
        write_config(CONFIG.replace("never_switch_resolution = false", "never_switch_resolution = true"))
        gdbus("Reload")
        time.sleep(0.5)
        window("windowed")
        check(wait_for(lambda: is_ours(state()["DP-1"]), 10), "vibrance ainda é aplicado")
        check(state()["DP-1"]["currentModeId"] == "1", "resolução NÃO foi trocada")
        window("firefox")
        wait_for(lambda: not is_ours(state()["DP-1"]), 10)

        print("== somente monitor principal")
        write_config(CONFIG.replace("affect_primary_only = false", "affect_primary_only = true"))
        gdbus("Reload")
        time.sleep(0.5)
        window("cs2", output="HDMI-A-1")
        time.sleep(1.5)
        check(not is_ours(state()["HDMI-A-1"]) and not is_ours(state()["DP-1"]),
              "jogo no monitor secundário não altera nada")
        window("cs2", output="DP-1")
        check(wait_for(lambda: is_ours(state()["DP-1"]), 10), "jogo no principal aplica vibrance")
        window("firefox")
        wait_for(lambda: not is_ours(state()["DP-1"]), 10)

        print("== nível do desktop")
        write_config(CONFIG.replace("windows_level = 50", "windows_level = 60"))
        gdbus("Reload")
        check(wait_for(lambda: is_ours(state()["DP-1"]) and is_ours(state()["HDMI-A-1"]), 10),
              "nível 60% aplicado em todos os monitores")
        check(state()["DP-1"]["iccProfilePath"].endswith("-60.icc"), "perfil do nível 60%")
        window("cs2")
        check(wait_for(lambda: state()["DP-1"]["iccProfilePath"].endswith("-80.icc"), 10), "jogo sobe para 80% em DP-1")
        check(state()["HDMI-A-1"]["iccProfilePath"].endswith("-60.icc"), "HDMI-A-1 fica em 60%")

        print("== desativar o observador")
        gdbus("SetEnabled", "false")
        check(wait_for(lambda: not is_ours(state()["DP-1"]) and not is_ours(state()["HDMI-A-1"]), 10),
              "tudo volta ao normal")
        gdbus("SetEnabled", "true")
        check(wait_for(lambda: is_ours(state()["DP-1"]), 10), "reativar volta a aplicar")

        print("== pré-visualização ao vivo")
        window("firefox")
        write_config(CONFIG)  # desktop 50
        gdbus("Reload")
        wait_for(lambda: not is_ours(state()["DP-1"]), 10)
        gdbus("Preview", "90")
        check(wait_for(lambda: state()["DP-1"]["iccProfilePath"].endswith("-90.icc"), 10), "preview aplica 90%")
        gdbus("Preview", "int32 -1")
        check(wait_for(lambda: not is_ours(state()["DP-1"]), 10), "fim do preview restaura")

        print("== encerramento limpo restaura os monitores")
        window("cs2")
        wait_for(lambda: is_ours(state()["DP-1"]), 10)
        daemon.send_signal(signal.SIGTERM)
        try:
            daemon.wait(timeout=20)
        except subprocess.TimeoutExpired:
            daemon.kill()
        check(daemon.returncode == 0, f"daemon encerrou com código 0 (foi {daemon.returncode})")
        check(not is_ours(state()["DP-1"]), "SIGTERM restaura DP-1")
        check(state()["DP-1"]["source"] == "sRGB", "fonte de cor original")

        print("== recuperação de sessão interrompida (perfil nosso já aplicado)")
        subprocess.run([BIN, "set", "DP-1", "70"], env=env, check=True, capture_output=True)
        check(is_ours(state()["DP-1"]), "pré-condição: DP-1 ficou com perfil (simula crash)")
        daemon = subprocess.Popen([BIN, "--minimized"], env=env, stdout=log, stderr=subprocess.STDOUT)
        check(wait_for(lambda: not is_ours(state()["DP-1"]), 15), "novo daemon limpa o perfil antigo ao iniciar")
        daemon.send_signal(signal.SIGTERM)
        daemon.wait(timeout=20)

        print("== CLI")
        r = subprocess.run([BIN, "set", "HDMI-A-1", "85"], env=env, capture_output=True, text=True)
        check(r.returncode == 0 and is_ours(state()["HDMI-A-1"]), "set aplica nível manual")
        r = subprocess.run([BIN, "reset"], env=env, capture_output=True, text=True)
        check(r.returncode == 0 and not is_ours(state()["HDMI-A-1"]), "reset remove o vibrance")
        r = subprocess.run([BIN, "set", "DP-1", "30"], env=env, capture_output=True, text=True)
        check(r.returncode != 0, "nível fora da faixa é rejeitado")
        r = subprocess.run([BIN, "set", "XX-9", "70"], env=env, capture_output=True, text=True)
        check(r.returncode != 0, "monitor inexistente é rejeitado")
    finally:
        for p in (daemon, kwin):
            if p.poll() is None:
                p.kill()
        log.close()

    print()
    if failures:
        print(f"{len(failures)} verificação(ões) falharam:")
        for f in failures:
            print("  -", f)
        print("\n--- fim do log do daemon ---")
        print("".join(open(os.path.join(TMP, "daemon.log")).readlines()[-40:]))
        sys.exit(1)
    print("todas as verificações passaram")


if __name__ == "__main__":
    main()

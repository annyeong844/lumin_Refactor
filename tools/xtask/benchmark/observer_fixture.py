"""Native process fixtures; never imported or invoked by the measured product."""

import importlib.util
import os
import socket
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True


def load_observer():
    spec = importlib.util.spec_from_file_location("observer", Path(__file__).with_name("measure-process.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def product(mode, arguments):
    if mode in ("child", "grandchild", "terminated-child"):
        child_mode = "child" if mode == "grandchild" else "clean"
        subprocess.run([sys.executable, "-I", "-S", __file__, "product", child_mode],
                       stdout=subprocess.DEVNULL, check=True)
    elif mode == "lingering":
        subprocess.Popen([sys.executable, "-I", "-S", __file__, "product", "held-child", arguments[0]],
                         stdout=subprocess.DEVNULL)
        with socket.create_connection(("127.0.0.1", int(arguments[0]))) as connection:
            connection.sendall(f"root {os.getpid()}\n".encode())
            if connection.recv(1) != b"R":
                raise RuntimeError("missing root release")
    elif mode == "held-child":
        with socket.create_connection(("127.0.0.1", int(arguments[0]))) as connection:
            connection.sendall(f"child {os.getpid()}\n".encode())
            if connection.recv(1) != b"X":
                raise RuntimeError("child exited without designated release")
    elif mode != "clean":
        raise RuntimeError(f"unknown fixture mode {mode}")
    sys.stdout.buffer.write(b"{}\n")


if __name__ == "__main__":
    if sys.argv[1] == "product":
        product(sys.argv[2], sys.argv[3:])
    elif sys.argv[1] in ("nested-helper", "poisoned-tree-helper"):
        observer = load_observer()
        outer = None
        if sys.argv[1] == "nested-helper":
            # This fixture is disposable; the unittest runner is never assigned.
            outer = observer.WindowsJob()
            outer.admit()
        else:
            def forbidden(*args):
                raise AssertionError("Windows consulted global PID ancestry")
            observer.linux_process_tree = forbidden
            observer.descendants = forbidden
            # PID 10 is reused; 20 belongs to its former lifetime, 21 is younger.
            observer.windows_process_tree = lambda: {20: 10, 21: 20, 30: 10}
        sys.argv = [str(observer.__file__)] + sys.argv[2:]
        try:
            observer.main()
        finally:
            if outer is not None:
                outer.close()
    else:
        raise RuntimeError("unknown fixture role")

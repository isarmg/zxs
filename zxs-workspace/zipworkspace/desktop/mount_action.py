"""An explicitly reviewed, cancellable handoff to this installed project's Linux FUSE helper."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import shlex
import socket
import stat
import subprocess
import sys
import threading

from ..archive import stamp
from .._mount_child import mount_receipt, mountinfo_path
from ..document import native_identity
from ..linux_authority import ProcView, owned_directory
from ..linux_mount import _ordinary_user, held_point, point_path
from ..model import Rejected, new_id
from ..private_freeze import mount_id

MOUNTINFO_BUDGET = 2 << 20


def mount_presence(rows, point, token, parent_id, original_id):
    """An exact original mount, absence, or conflicting/incomplete evidence."""
    if original_id == parent_id: raise Rejected("原挂载 ID 不能也是自己的父挂载")
    if not 0 < len(rows) <= MOUNTINFO_BUDGET or not rows.endswith(b"\n"):
        raise Rejected("内核挂载表没有完整读回")
    path, source = mountinfo_path(point), ("zip-workspace-ro-" + token).encode("ascii")
    related = []
    for row in rows.split(b"\n")[:-1]:
        head, sep, tail = row.partition(b" - ")
        a, b = head.split(b" "), tail.split(b" ")
        if not sep or len(a) < 6 or len(b) < 3 or not a[0].isdigit() or not a[1].isdigit():
            raise Rejected("不能完整辨认本次挂载的内核记录")
        if a[0] == original_id or a[4] == path or b[1] == source:
            related.append(row)
    if not related: return "absent"
    if len(related) == 1 and mount_receipt(related[0], point, token, parent_id) == original_id:
        return "present"
    return "unknown"  # The ID may already have been reused; the point might hold another mount.


class MountChoice:
    def __init__(self, snapshot, spelling):
        self.stack, self.snapshot = ExitStack(), snapshot
        self.ready = self.used = self.closed = False
        if sys.platform != "linux" or snapshot.held is None or snapshot.held.closed:
            raise Rejected("请在 Linux 窗口重新打开快照，以保留实际核验的原归档")
        if not spelling.isprintable(): raise Rejected("请选可直接显示的挂载路径")
        self.path = point_path(spelling)
        self.token = new_id()
        try:
            self.uid = _ordinary_user()
            self.leaf = os.open(self.path, os.O_PATH | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
            self.stack.callback(os.close, self.leaf)
            src = snapshot.held.duplicate()
            self.stack.callback(os.close, src)
            self.doc = self.stack.enter_context(os.fdopen(src, "rb", closefd=False))
        except BaseException:
            self.close(); raise

    def close(self):
        self.closed = True
        self.stack.close()

    def review(self):
        if self.closed or self.ready: raise Rejected("本次选择已失效")
        s = self.snapshot
        _, self.fds, check = self.stack.enter_context(held_point(str(self.path), self.uid, leaf=self.leaf))
        if (stamp(self.doc) != s.observed or
                native_identity(s.path.stat(follow_symlinks=False), stat.S_IFREG) != s.observed[:2] or
                s.exact is not None and (s.exact.path != s.path or s.exact.observed != s.observed or
                                         s.exact.marker != s.browser.commit)):
            raise Rejected("选择已变化；未按名称重选另一个归档")
        check()
        self.ready = True
        mark = s.browser.commit
        return (f"原归档：{os.fspath(s.path)!r}\n节点：{mark.operation}　分支：{mark.branch}　第 {mark.generation} 代\n"
                f"整树 SHA-256：{mark.tree}\n已核成员数据：{s.browser.data_bytes:,} 字节\n"
                f"原空挂载点：{self.path}\n确认后助手才从同一原件核验全部内容并建立索引。"
                "核验期间可取消；未建索引或未通过资格不会直接挂载。仅只读，本机同用户其他程序可访问。")

    def launch(self, *, confirmed=False):
        if not confirmed or not self.ready or self.used or self.closed: raise Rejected("缺少本次挂载的明确确认")
        self.used = True
        try:
            # An opaque mount label has no decision authority, but lets the operator distinguish this mount.
            s = self.snapshot
            helper = str(Path(__file__).resolve().parents[1] / "_mount_child.py")
            req = {"point": str(self.path), "name": s.path.name, "observed": s.observed,
                   "marker": s.browser.commit.encode().decode("ascii"), "token": self.token,
                   "sha": s.exact.sha256 if s.exact is not None else None}
            packet = json.dumps(req, ensure_ascii=True, separators=(",", ":")).encode("ascii")
            if len(packet) > 16384 or not os.path.isabs(sys.executable): raise Rejected("无法建立有界的本机助手交接")
            parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
            witness = None
            try:
                with child:
                    witness = os.dup(self.leaf)  # Keep only the original empty point, not a second archive FD.
                    parent.setblocking(False)
                    if parent.send(packet) != len(packet): raise Rejected("未完整交接本次请求")
                    fds = (child.fileno(), self.doc.fileno(), *self.fds)
                    # Trust the chosen Python installation/dependencies, but do not resolve this
                    # helper via cwd, PATH, PYTHONPATH or another installed copy of our project.
                    proc = subprocess.Popen([sys.executable, "-I", "-B", helper, *map(str, fds)], cwd="/", shell=False, close_fds=True,
                                            pass_fds=fds, start_new_session=True, stdin=subprocess.DEVNULL,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                return MountProcess(proc, parent, self.path, self.token, witness=witness, uid=self.uid)
            except BaseException:
                parent.close()
                if witness is not None: os.close(witness)
                raise
        finally:
            self.close()


class MountProcess:
    """Single GUI-thread owner. Never kill, unmount, or issue a second start."""
    def __init__(self, proc, link, point, token, *, witness=None, uid=None):
        self.proc, self.link, self.point, self.token = proc, link, point, token
        self.witness, self.uid = witness, uid
        self.ready = self.sent = self.cancelled = self.exited = self.accepted = False
        self.mid = self.error = None
        self.refused = False

    def _disconnect(self):
        if self.link is not None: self.link.close(); self.link = None

    def close_observation(self):
        fd, self.witness = self.witness, None
        if fd is not None: os.close(fd)

    def cancel(self):
        if self.sent: return False
        self.cancelled = True
        self._disconnect()  # This process never sent G; its helper cannot pass authorize().
        self.close_observation()
        return True

    def grant(self):
        if self.sent or not self.ready or self.cancelled or self.exited or self.error or self.link is None:
            raise Rejected("本次挂载授权已失效")
        self.sent = True  # A lost reply must never authorize another request.
        try:
            if self.link.send(b"G") != 1: raise OSError("incomplete mount handoff")
        except OSError as err:
            self.error = "挂载授权交接结果未知：" + str(err)
            self._disconnect()

    def _read(self):
        while self.link is not None:
            try: msg = self.link.recv(768)
            except BlockingIOError: return
            except OSError:
                self._disconnect(); return
            if not msg:
                self._disconnect(); return
            if msg == b"R" and not self.ready and not self.sent: self.ready = True
            elif msg == b"I" and self.sent: pass
            elif self.sent and msg == b"A": self.accepted = True
            elif self.sent and msg[:1] == b"M" and 1 <= len(msg[1:]) <= 20 and msg[1:].isdigit():
                self.accepted = True; self.mid = msg[1:].decode("ascii")
            elif msg[:1] in (b"E", b"U") and len(msg) <= 501:
                self.refused = msg.startswith(b"E")
                self.error = ("核验未通过，助手未调用系统挂载：" if self.refused else "系统挂载结果待核：") + msg[1:].decode("utf-8", "replace")
            else:
                self.error = "助手返回未知状态；不会重试。"
                self._disconnect()

    def poll(self):
        self._read()
        if self.proc.poll() is not None:
            self._read()  # Drain the real last packet even when exit and poll cross.
            self.exited = True
            self._disconnect()

    def leave(self):
        if not self.sent: self.cancel()
        self._disconnect()
        self.close_observation()
        if self.proc.poll() is None:
            threading.Thread(target=self.proc.wait, name="reap-mount-helper", daemon=True).start()

    def instructions(self):
        path = shlex.quote(str(self.point))
        label = "zip-workspace-ro-" + self.token
        ident = f"，ID 必须为 {self.mid}" if self.mid is not None else ""
        return (f"在同一挂载命名空间另开终端，先执行：\nfindmnt --kernel --nocanonicalize --mountpoint {path} "
                f"--output ID,TARGET,FSTYPE,SOURCE\n核对仅一项、类型为 fuse 或 fuse.*、SOURCE 为 {label}{ident}。"
                f"只有仍是本次挂载才关闭占用并正常卸载：\nfusermount3 -u -- {path}\n不匹配或无法确认时不要卸载。")

    @property
    def can_check_exit(self):
        return self.exited and self.sent and self.mid is not None and self.witness is not None

    def check_exit(self):
        if not self.can_check_exit:
            raise Rejected("没有尚可核对身份的原挂载")
        with ExitStack() as hold:
            proc = ProcView(hold)
            before = os.fstat(self.witness)
            parent = mount_id(self.witness, read_proc=proc.read)
            expected = (before.st_dev, before.st_ino, parent)
            def observe():
                rows = proc.read("mountinfo", MOUNTINFO_BUDGET)
                return mount_presence(rows, self.point, self.token, parent, self.mid.encode("ascii"))
            visible = observe()
            if visible == "present":
                return ("助手已退出，内核在本挂载命名空间仍列出这笔原只读挂载。\n" + self.instructions())
            if visible == "unknown":
                raise Rejected("出现其他挂载、重复名称或已用作别处的 ID；不判断是否已卸载")
            # No stat through a live FUSE mount is needed. At most get metadata
            # handles for the actual old directory chain, then re-read the table.
            _, identities = owned_directory(hold, self.point, self.uid, proc.read, metadata_only=True)
            if identities[-1] != expected or identities[-2][2] != parent or observe() != "absent":
                raise Rejected("挂载点、父挂载或内核记录已变化；未确认卸载")
        return ("本挂载命名空间已不见这笔原只读挂载，并核对到原目录重新可见。"
                f"\n原挂载 ID：{self.mid}\n原位置：{self.point}")

    def status(self):
        if self.exited:
            if self.cancelled: return "已取消；未向助手发出挂载授权。"
            if self.refused or not self.sent: return self.error or "助手未完成核验，未授权挂载。请核对同一安装环境及可选依赖。"
            return (self.error + "\n" if self.error else "") + "助手已退出；未据此认定已经卸载。\n" + self.instructions()
        if self.cancelled: return "正在取消只读核验；未发出挂载授权。"
        if self.error: return self.error + ("\n" + self.instructions() if self.sent else "")
        if self.mid: return f"已在内核表核到本次只读挂载（当时 ID {self.mid}）。\n" + self.instructions()
        if self.accepted: return "FUSE 已接受请求；未核实内核挂载表，不判断当前挂载状态。\n" + self.instructions()
        if self.sent: return "正在请求系统挂载，尚未核实结果。\n" + self.instructions()
        if self.ready: return "归档已核验，等待当前窗口操作结束；仍可取消，尚未请求挂载。"
        return "正在核验归档并建立块索引，可取消；尚未向系统请求挂载。"

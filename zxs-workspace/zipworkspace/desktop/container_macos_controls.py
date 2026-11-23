"""Ordinary Mac desktop: genuine original mount, normal stop, full-file review, explicit remote action."""
from concurrent.futures import ThreadPoolExecutor
from functools import partial

from PySide6.QtCore import QTimer, Qt
from PySide6.QtWidgets import QDialog, QHBoxLayout, QLineEdit, QPlainTextEdit, QVBoxLayout, QWidget

from .._macos_fskit_release import packaged, require_release
from ..macos_drive_owner import archive, captured, original_save, recover, registered, reserve
from ..macos_fskit_handoff import CompletedDrive
from ..macos_fskit_registration import SelectionRequest
from ..macos_retained_submit import Phase, retry_original, status_original, submit_original
from ..model import Rejected
from .macos_fskit_host import MountPhase, OriginalDrive
from .widgets import button, confirm, label


class MacDiskControls(QWidget):
    def __init__(self, host, panel):
        super().__init__(panel)
        self.host, self.panel, self.active = host, panel, None
        layout=QVBoxLayout(self)
        layout.addWidget(label("macOS 本机磁盘：打开前核对本机签名版本和系统；只有明确提交才会上传。"
                               if packaged() else "本版本未包含通过原机验收的 macOS 可写磁盘。仍可浏览远端容器。"))
        row=QHBoxLayout()
        self.edit=button("将所选节点作为本机磁盘打开…", lambda: self.open(False))
        self.recover=button("核对原磁盘及提交…", lambda: self.open(True))
        row.addWidget(self.edit); row.addWidget(self.recover); layout.addLayout(row)

    def scope(self, cold):
        p, conn=self.panel, self.panel._owned
        cid=p._current()
        if (not packaged() or conn is None or conn.writable or not conn._listed or conn.operations is not p.ops
                or p._checking_connection or p.jobs or self.host.pending is not None or self.host._container_edit_locked):
            return None
        if cold:
            return conn,p._epoch,cid if cid in p.listed else None,None,None,None
        if cid not in p.listed: return None
        info,branch,node=p.details.get(cid),p.branches.currentData(),p.nodes.currentData()
        if (info is None or info.container!=p.listed[cid] or not info.container.revision or branch not in info.branches
                or node is None or node not in branch.nodes or node.id!=branch.head and branch.id!="main"):
            return None
        return conn,p._epoch,cid,info,branch,node

    def paint(self):
        self.edit.setEnabled(not self.busy() and self.scope(False) is not None)
        self.recover.setEnabled(not self.busy() and self.scope(True) is not None)

    def busy(self):
        return self.active is not None

    def finish(self):
        if self.busy():
            raise Rejected("请先完成关闭 macOS 原生磁盘窗口。")

    def open(self,cold):
        selected=self.scope(cold)
        if self.busy() or selected is None:
            return
        box=self.active=_Disk(self,selected,cold)
        self.panel._paint()
        def done(_):
            if self.active is box:
                self.active=None
                if box.invalidate:
                    self.panel.details.pop(box.cid,None)
                    self.panel._select()
                self.panel._paint()
            box.deleteLater()
        box.finished.connect(done); box.open()


class _Disk(QDialog):
    def __init__(self,controls,selected,cold):
        super().__init__(controls.panel)
        self.controls,self.selected,self.cold=controls,selected,cold
        self.conn,_,self.cid,info,branch,node=selected
        self.setWindowTitle("核对原磁盘及提交" if cold else "macOS 原生工作磁盘")
        self.setWindowModality(Qt.WindowModality.WindowModal); self.resize(650,510)
        self.pool=ThreadPoolExecutor(max_workers=1,thread_name_prefix="workspace-macos-drive")
        self.job=self.task=self.request=self.route=self.known=self.drive=self.completed=None
        self.cap=self.choice=self.observed=self.accepted=None
        self.state="setup"; self.detach_once=self.invalidate=self.ended=self.closed=False
        outer=QVBoxLayout(self)
        line=f"服务器 {self.conn._exchange._location!r}　账号 {self.conn._account!r}"
        if not cold: line+=f"\n容器 {self.cid}"
        if info:
            line+=f"　{info.container.name!r}　修订 {info.container.revision}\n分支 {branch.id!r}　原节点 {node.id}　{node.length:,} 字节"
        outer.addWidget(label(line))
        self.token=QLineEdit(self); self.cid_input=QLineEdit(self)
        self.token.setVisible(cold); self.cid_input.setVisible(cold)
        if cold:
            self.cid_input.setPlaceholderText("原容器 ID（32 位，可从之前的记录复制）")
            self.cid_input.setText(self.cid or "")
            outer.addWidget(self.cid_input)
            self.token.setPlaceholderText("活动原件留空；核对已归档的磁盘时填入原 32 位磁盘号")
            outer.addWidget(self.token)
        self.text=QPlainTextEdit(); self.text.setReadOnly(True); outer.addWidget(self.text)
        row=QHBoxLayout()
        self.action=button("",self.primary); self.query=button("只查这笔原回执",self.lookup); self.exit=button("关闭",self.reject)
        for w in (self.action,self.query,self.exit): row.addWidget(w)
        outer.addLayout(row)
        self.timer=QTimer(self); self.timer.setInterval(250); self.timer.timeout.connect(self.poll)
        self.note("只读取本机原号，不扫描目录；不会重新挂载或自动发送。" if cold else
                  "在 Finder 中原位编辑所选文件。临时文件后替换、增删、改名和执行程序尚不支持。"
                  "下载到应用的本机私有 APFS 目录；不改远端名称。关闭编辑器和 Finder 窗口后正常卸载，"
                  "再核对是否提交。应用不安装扩展、不强制卸载。")
        self.paint()

    def note(self,msg):
        if self.text.toPlainText()!=msg: self.text.setPlainText(msg)

    def context(self):
        now=self.controls.scope(self.cold)
        return now is not None and now[0] is self.conn and now[1:]==self.selected[1:]

    def tail(self):
        token=self.known.token if self.known else self.request.token if self.request else None
        return "\n原磁盘号："+token if token else ""

    def describe(self):
        a=self.choice.action
        return (f"原分支 {self.route.branch!r}，原节点 {self.route.node}，基础修订 {a.base_revision}\n"
                f"目标分支 {a.branch!r}，新节点 {a.node_id}"
                + (f"（从主分支历史节点 {a.fork_from_main} 分出）" if a.fork_from_main else "") +
                f"\n完整正文 {a.body.length:,} 字节　SHA-256 {a.body.sha256}\n原提交 {self.choice.key.submission}\n"
                f"原操作 {a.operation_id}"+self.tail())

    def paint(self):
        names={"setup":"读取原记录" if self.cold else "下载并打开…","mount":"正常卸载",
               "busy":"再次尝试正常卸载","normal":"核对并保留完整修改","cold_normal":"核对原来的正常卸载",
               "ready":"提交完整文件…","absent":"发送同一笔原提交","settled":"归档本机记录","unchanged":"归档未修改的磁盘"}
        self.action.setText(names.get(self.state,"等待"))
        self.action.setEnabled(self.job is None and self.state in names)
        self.query.setEnabled(self.job is None and self.choice is not None and self.state not in ("done","fault"))
        self.token.setEnabled(self.job is None and self.state=="setup")
        self.cid_input.setEnabled(self.job is None and self.state=="setup")
        self.exit.setEnabled(self.job is None and self.state not in ("attaching","detach","mount","busy","drain"))

    def work(self,kind,fn):
        if self.job: raise Rejected("正在处理原操作。")
        self.task,self.job=kind,self.pool.submit(fn)
        self.timer.start(); self.paint()

    def primary(self):
        if self.job is not None: return
        try:
            if self.state in ("mount","busy"):
                if self.state=="mount": self.drive.stop_new_opens(confirmed=True)
                self.detach_once=True; self.state="drain"; self.timer.start(); self.paint(); return
            if not self.context(): raise Rejected("连接或原选择已改变；不启动新操作。")
            if self.state=="setup":
                if self.cold:
                    self.cid=self.cid_input.text().strip()
                    token=self.token.text().strip() or None
                    self.ended=token is not None
                    def read():
                        require_release()
                        return recover(self.conn,self.cid,ended_token=token)
                    self.work("recover",read)
                else:
                    _,_,_,info,branch,node=self.selected
                    if not confirm(self,"以本机磁盘打开",
                            f"下载 {info.container.name!r} 的原节点 {node.id}（{node.length:,} 字节）至应用的私有 APFS，"
                            "至少留出 256 MiB 余量，再请已启用的签名 FSKit 扩展在 /Volumes 呈现可写卷。不会上传。",
                            attestation="我选择这个原节点，并允许本次本地下载和挂载。",action_text="下载并挂载") or not self.context(): return
                    self.request=SelectionRequest(self.conn,((info,branch,node),),confirmed=True)
                    self.work("release",require_release)
            elif self.state in ("normal","cold_normal"):
                if self.completed is None:
                    self.completed=(CompletedDrive.after_normal(self.drive,self.conn) if self.drive else
                                    CompletedDrive.reopen(self.known,self.conn,confirmed=True))
                loan=self.completed.borrow(self.conn,self.route.source,confirmed=True)
                try:
                    def keep():
                        kept=loan.retain(self.conn,self.route.root,confirmed=True)
                        return captured(self.conn,self.route,self.known,kept)
                    self.work("capture",keep)
                except BaseException:
                    loan.cancel_unclaimed(); raise
            elif self.state in ("ready","absent"):
                self.send()
            elif self.state in ("unchanged","settled"):
                self.work("archive",partial(archive,self.conn,self.cap,self.observed if self.cap.key else None))
        except Exception as e: self.fault(e)

    def send(self):
        if self.choice is None or self.ended or (self.observed and self.observed.phase is Phase.CONFIRMED): return
        retry=self.state=="absent" and self.observed.attempted
        if self.accepted!=self.choice:
            # This is the single confirmation of this exact action, never an extra modal solely for retry.
            if not confirm(self,"提交原完整文件",self.describe(),attestation="确认向上述原分支提交这份完整正文。",
                           action_text="按原号提交") or not self.context(): return
            self.accepted=self.choice
        def send():
            require_release()
            if original_save(self.conn,self.cap)!=self.accepted: raise Rejected("已确认的原操作发生变化。")
            return retry_original(self.conn,self.observed,confirmed=True) if retry else submit_original(self.conn,self.choice,confirmed=True)
        self.work("send",send)

    def lookup(self):
        if self.job or not self.choice or not self.context(): return
        def query():
            require_release()
            if original_save(self.conn,self.cap)!=self.choice: raise Rejected("原提交或来源发生变化。")
            return status_original(self.conn,self.choice,confirmed=True)
        self.work("query",query)

    def fault(self,e):
        self.state="pending" if self.choice is not None else "fault"
        self.note(f"此步未获确认：{e}\n保留原件，不按名称重建、不自动尝试挂载或上传。"+self.tail())
        self.timer.stop(); self.paint()

    def set_capture(self,cap,hot):
        self.cap=cap
        if cap.key is None:
            self.state="done" if self.ended else "unchanged"
            self.note(f"整份文件与原节点一致：{cap.body.length:,} 字节，SHA-256 {cap.body.sha256}。"
                      "未创建新节点或提交。"+self.tail())
        else:
            self.work("review_new" if hot else "review_cold",partial(original_save,self.conn,cap))

    def poll(self):
        try:
            if self.job is not None:
                if not self.job.done(): return
                job,task=self.job,self.task; self.job=self.task=None
                r=job.result()
                if task=="release":
                    if not self.context(): raise Rejected("原连接或选择已更改。")
                    self.request.preflight(start=True); self.state="preflight"
                elif task=="prepare":
                    self.route,reg,self.known=r
                    if not self.context(): raise Rejected("已保留原件，但原选择改变，未发起挂载。")
                    self.drive=OriginalDrive(self.request,reg)
                    try: self.drive.attach(confirmed=True)
                    except Exception:
                        self.drive.release_unstarted(); raise
                    self.state="attaching"; self.note("已请求系统挂载，正在核对实际原卷。"+self.tail())
                elif task=="capture":
                    self.completed.release(confirmed=True); self.completed=None
                    self.set_capture(r,True)
                elif task=="recover":
                    self.route,self.known,cap=r
                    if cap is None:
                        self.state="cold_normal"; self.note("保留了原登记，尚无完整交接记录。只能核对原正常卸载，不能重挂载。"+self.tail())
                    else: self.set_capture(cap,False)
                elif task in ("review_new","review_cold"):
                    self.choice=r; self.state="ready" if task=="review_new" else "pending"
                    self.note(("本机已保留整份修改；尚未发送。\n" if task=="review_new" else "找到原操作，先只查原回执。\n")+self.describe())
                elif task in ("send","query"):
                    if self.observed and self.observed.phase is Phase.CONFIRMED and self.observed.fence!=r.fence:
                        raise Rejected("后来的结果与已确认回执不同；不能再次发送。")
                    self.observed=r
                    if r.phase is Phase.CONFIRMED:
                        self.invalidate=True; self.state="done" if self.ended else "settled"
                        self.note(f"服务器已确认原操作：修订 {r.fence.sequence}，分支 {r.fence.branch!r}，新节点 {r.fence.head}。"+self.tail())
                    elif r.phase is Phase.ABSENT:
                        self.state="pending" if self.ended else "absent"
                        self.note("已认证的服务器明确没有原号回执。只能选择同一笔；发送前还会复查。\n"+self.describe())
                    elif r.phase is Phase.CONFLICT:
                        self.state="pending"; self.invalidate=True
                        self.note("基础修订或分支头冲突，不覆盖、不换号。\n"+self.describe())
                    else:
                        self.state="pending"; self.note("结果未知。请只查原号，不能从此结果推定未上传。\n"+self.describe())
                elif task=="archive":
                    self.state="done"; self.ended=True
                    self.note("已归档本机原记录，可用原磁盘号核对；未删除原件。"+self.tail())
            elif self.state=="preflight":
                if not self.request.preflight(): return
                def prepare():
                    route=reserve(self.conn,self.request)  # Durable original token before any body GET.
                    reg=self.request.prepare()
                    return route,reg,registered(self.conn,route,reg)
                self.state="prepare"; self.note("正在取得原节点并核对整份文件，还没有挂载。"+self.tail())
                self.work("prepare",prepare)
            elif self.state in ("attaching","mount","drain","busy","detach","gone"):
                self.observe()
            if self.job is None and self.state not in ("preflight","attaching","mount","drain","busy","detach","gone"):
                self.timer.stop()
            self.paint()
        except Exception as e: self.fault(e)

    def observe(self):
        status=self.drive.status(); phase=status.phase
        if phase is MountPhase.READY:
            if status.errno:
                self.drive.release_unstarted()
                raise Rejected(f"未发起挂载，系统错误 {status.errno}。")
        elif phase is MountPhase.UNKNOWN:
            raise Rejected(f"原系统挂载状态不可确认（{status.errno}），不能提交。")
        elif phase is MountPhase.MOUNTED:
            self.state="mount"
            self.note("磁盘已由系统挂载。可在 Finder 的「前往文件夹」输入：\n"+status.mounted_path+
                      "\n编辑完成后关闭文件和该目录的 Finder 窗口，再正常卸载。未上传。"+self.tail())
        elif phase in (MountPhase.STOPPING,MountPhase.STOPPED_READY):
            if phase is MountPhase.STOPPED_READY and self.detach_once:
                self.detach_once=False; self.drive.normal_unmount(confirmed=True); self.state="detach"
                self.note("正在请求对原卷的正常卸载；不会强制结束。"+self.tail())
            elif self.state=="detach":
                self.state="busy"; self.note("系统未卸载原卷。请关闭仍占用文件或目录的应用，再手动重试。"+self.tail())
            elif self.state!="busy":
                self.note("已停接新的文件打开。正在等已有引用关闭及全部文件完成本机同步。"+self.tail())
        elif phase is MountPhase.GONE_WAITING_PROVIDER:
            self.state="gone"; self.note("原卷已从系统卸载；尚无提供方的正常完成证明，不能提交。"+self.tail())
        elif phase is MountPhase.LOCAL_NORMAL:
            self.state="normal"; self.note("已核实双方正常卸载。核对完整修改只写本地；仍不会上传。"+self.tail())

    def reject(self):
        if self.closed: return super().reject()
        if self.job is not None or self.state in ("attaching","mount","busy","drain","detach"): return
        if self.state=="preflight":
            try: self.request.cancel_preflight()
            except Exception as e: return self.fault(e)
        if self.state in ("normal","cold_normal","gone","fault"):
            if not confirm(self,"保留原件并关闭",
                    "不会上传、强制卸载或删除。未取得双方正常卸载证明的磁盘，之后不能仅凭路径或消失的盘恢复提交。"+self.tail(),
                    attestation="保留原记录，不把未确认状态当作成功。",action_text="保留并关闭"): return
        try:
            if self.completed is not None:
                self.completed.park(confirmed=True); self.completed=None
            elif self.drive is not None and self.state in ("normal","gone"):
                self.drive.park_system_gone(confirmed=True)
        except Exception as e: return self.fault(e)
        self.timer.stop(); self.pool.shutdown(wait=False); self.closed=True
        super().reject()

    def closeEvent(self,event):
        if self.closed: event.accept()
        else: event.ignore(); self.reject()

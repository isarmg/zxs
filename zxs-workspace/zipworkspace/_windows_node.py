"""Internal Windows export of one exact R3 node; no archive, UI, or mount."""
import ctypes as c
from contextlib import ExitStack
from threading import Lock

from ._transfer import CancelledCopy
from ._windows_access import OutputProtection
from ._windows_origin import DATA, ZONE
from ._windows_output import PinnedParent, _NTError, leaf
from .container_https import HttpsExchange
from .model import CHUNK, Rejected, new_id
from .publish import Uncertain
from .windows_nt import I32, P, U32, _function


_MUTZ_DONT_USE_CACHE = 0x1000


def _preserves_zone():
    # AttachmentManager/DoNotPreserveZoneInformation: user policy, absent means keep.
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                            r"Software\Microsoft\Windows\CurrentVersion\Policies\Attachments",
                            0, winreg.KEY_QUERY_VALUE) as key:
            value, kind = winreg.QueryValueEx(key, "SaveZoneInformation")
    except FileNotFoundError:
        return True
    if kind != winreg.REG_DWORD or value not in (1, 2):
        raise Rejected("无法核清 Windows 下载来源记录策略。")
    return value == 2


def _map_zone(url):
    # Use the system default URL security manager, not hard-coded Internet zoning.
    ole = c.WinDLL("ole32.dll", winmode=0x800)
    urlmon = c.WinDLL("urlmon.dll", winmode=0x800)
    initialize = _function(ole, "CoInitializeEx", I32, P, U32)
    uninitialize = _function(ole, "CoUninitialize", None)
    create = _function(urlmon, "CoInternetCreateSecurityManager", I32, P, c.POINTER(P), U32)
    rc = initialize(None, 0x2 | 0x4)  # COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE
    if rc not in (0, 1) and rc & 0xffffffff != 0x80010106:  # already in a different apartment
        raise Rejected("无法初始化 Windows 下载来源判定。")
    try:
        manager = P()
        if create(None, c.byref(manager), 0) != 0 or not manager.value:
            raise Rejected("无法使用 Windows URL 安全区域判定。")
        # IUnknown::Release and IInternetSecurityManager::MapUrlToZone, URLMON SDK ABI.
        table = c.cast(manager, c.POINTER(c.POINTER(P))).contents
        release = c.WINFUNCTYPE(U32, P)(table[2])
        try:
            classify = c.WINFUNCTYPE(I32, P, c.c_wchar_p, c.POINTER(U32), U32)(table[5])
            zone = U32(0xffffffff)
            rc_map = classify(manager, url, c.byref(zone), _MUTZ_DONT_USE_CACHE)
            if rc_map != 0 or zone.value not in range(5):
                raise Rejected("无法核清 Windows 对原 HTTPS 地址判定的安全区域。")
            return zone.value
        finally:
            release(manager)
    finally:
        if rc in (0, 1):  # Both S_OK and S_FALSE count toward initialization.
            uninitialize()


def _effective_zone(url):
    if not _preserves_zone():
        return None  # Explicitly suppressed. Distinct from any known Windows zone.
    zone = _map_zone(url)
    if not _preserves_zone():
        raise Rejected("Windows 下载来源记录策略在核对期间已经改变。")
    return zone


def _remote_mark(url, zone):
    # url is constructed from the original transport's validated HTTPS authority and GET target.
    # Do not synthesize a referrer, share credentials, or equate this record with antivirus approval.
    if zone is None or zone < 3:  # Local Machine, Intranet and Trusted Sites remain unmarked.
        return None
    return f"[ZoneTransfer]\r\nZoneId={zone}\r\nHostUrl={url}\r\n".encode("ascii")


def _same_file(parent, h, name, size, file_id=None):
    api, spelling = parent.api, parent.name(name)
    held = api.ordinary(h, parent.volume, spelling)
    if held.size != size or (file_id is not None and held.file_id != file_id):
        raise Rejected("本机结果已改变文件身份或长度。")
    with api.witness(parent.handle, name) as named:
        if api.ordinary(named, parent.volume, spelling) != held:
            raise Rejected("原本机名称不再指向仍持有的同一文件。")
    return held


def _check_origin(api, h, row, mark):
    if mark is None:
        api.only_default_stream(h, row.size)
        return
    if api.streams(h) != {DATA: row.size, ZONE: len(mark)}:
        raise Rejected("实际文件没有本次 Windows 策略要求的来源流。")
    volume, _ = api.info(h)
    with api.zone_witness(h) as held:
        zone_volume, zone = api.info(held)
        if (zone_volume != volume or zone.file_id != row.file_id or zone.directory
                or zone.size != len(mark) or api.read(held, len(mark)) != mark or api.read(held, 1)):
            raise Rejected("实际来源流与已核验的下载来源不符。")


def _noop(*_args):
    pass


class WindowsNodeOutput:
    """Hold two existing private output parents on the same fixed local NTFS.

    Construction only binds the explicit paths and checks access, without creating
    files or making requests. After confirmation, publish once with the original
    HttpsExchange and immutable node metadata. The caller's checkpoint must revalidate
    its live selection/connection and observe cancellation; begin_publication denies
    further cancellation. Once claimed, only the publishing worker owns the handles.
    Stage and destination must meet the original matching OutputProtection profile.
    The chosen private stage may retain incomplete bytes on failure; never recover,
    remove, or adopt a file by a later pathname.
    """

    def __init__(self, destination_parent, output_name, *, staging_parent):
        self._ownership, self._stack = Lock(), ExitStack()
        self._output = leaf(output_name)
        key = new_id().upper()
        self._stage = key[:8] + "." + key[8:11]  # Private 8.3 spelling, no secondary short name.
        try:
            self._destination = self._stack.enter_context(PinnedParent(destination_parent, output=True))
            self._staging = self._stack.enter_context(PinnedParent(staging_parent, output=True))
            dst, stage = self._destination, self._staging
            if (dst.guid != stage.guid or dst.volume != stage.volume
                    or dst.chain[-1][1] == stage.chain[-1][1]):
                raise Rejected("请选择同一本机固定 NTFS 卷上两个不同的私用目录。")
            self._protection = OutputProtection(stage.handle, dst.handle)
            dst.api.absent(dst.handle, self._output)
        except BaseException:
            self.close()
            raise

    def _take(self):
        # The same handoff linearizes an abandoned plan and a publishing worker.
        with self._ownership:
            stack, self._stack = self._stack, None
        return stack

    def close(self):
        """Abandon an unclaimed plan. To stop an active worker, request cancellation."""
        stack = self._take()
        if stack is not None:
            stack.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def _parents(self, output=None):
        self._staging.check()
        self._destination.check()
        self._protection.check(output)

    def publish(self, exchange, container, node, *, length, sha256, checkpoint, begin_publication,
                confirmed=False, progress=_noop, phase=_noop):
        if confirmed is not True:
            raise Rejected("请先确认本次原节点、暂存和正式目的文件。")
        stack = self._take()
        if stack is None:
            raise Rejected("这次原节点另存已关闭或已开始；请重新选择。")
        staged = may_be_published = False
        try:
            with stack, ExitStack() as opened:
                dst, stage, output, private = self._destination, self._staging, self._output, self._stage
                api, final_name, stage_name = dst.api, dst.name(output), stage.name(private)
                if type(exchange) is not HttpsExchange or exchange._authorization is None:
                    raise Rejected("另存需要这次已认证连接的原 HTTPS 读取器。")
                target = exchange._node_target(container, node, length, sha256)
                if length >= 1 << 63:  # Native FileStandardInformation has a signed 64-bit EOF.
                    raise Rejected("所选节点长度超出 NTFS 文件长度表示范围。")
                origin, identity = exchange._location, exchange.destination_identity

                def check():
                    checkpoint()
                    if (exchange._location != origin or exchange.destination_identity != identity):
                        raise Rejected("这次另存的原 HTTPS 地址或信任配置已改变。")

                url = origin + target
                check()
                policy_zone = _effective_zone(url)
                mark = _remote_mark(url, policy_zone)
                check()
                # Establish both original directory barriers before a private file receives bytes.
                api.flush(stage.handle)
                api.flush(dst.handle)
                self._parents()
                api.absent(dst.handle, output)
                check()
                held = opened.enter_context(api.new_file(stage.handle, private))
                staged = True
                original = _same_file(stage, held, private, 0)
                self._protection.check(held)
                api.only_default_stream(held, 0)
                if mark is not None:
                    zone = opened.enter_context(api.new_zone(held))
                    api.write(zone, mark)
                    api.flush(zone)
                    _check_origin(api, held, original, mark)

                def write(data):
                    # H2 can yield an empty frame; native writes are nonempty and bounded.
                    check()
                    for start in range(0, len(data), CHUNK):
                        block = data[start:start + CHUNK]
                        api.write(held, block)
                        progress(len(block))
                        check()
                    return len(data)

                phase("正在下载所选原节点到私用暂存文件", length)
                check()
                exchange.read_node_into(container, node, length=length, sha256=sha256, write=write)
                check()
                api.flush(held)
                sealed = _same_file(stage, held, private, length, original.file_id)
                phase("正在读回核验本机暂存文件", length)
                api.verify_bytes(held, length, sha256, checkpoint=check, on_read=progress)
                phase("正在复核 Windows 下载来源策略及两个原目录")
                check()
                # Reobserve supported OS policy on the same GET URL at the last
                # reversible boundary. Policy is not transactional with NTFS rename.
                if _effective_zone(url) != policy_zone:
                    raise Rejected("Windows 对原 HTTPS 地址的下载来源策略已经改变，没有发布文件。")
                check()
                if _same_file(stage, held, private, length, original.file_id) != sealed:
                    raise Rejected("已同步的本机暂存文件在核验期间改变。")
                _check_origin(api, held, sealed, mark)
                self._parents(held)  # Again after the potentially blocking policy lookup.
                check()
                begin_publication()
                may_be_published = True
                try:
                    api.rename_new(held, dst.handle, output)
                except _NTError as error:
                    if error.code != 0xc0000035:  # STATUS_OBJECT_NAME_COLLISION only.
                        raise
                    _same_file(stage, held, private, length, original.file_id)
                    may_be_published = False
                    raise Rejected("目的名称已被占用，没有覆盖。") from error
                api.flush(dst.handle)
                api.flush(stage.handle)
                actual = _same_file(dst, held, output, length, original.file_id)
                _check_origin(api, held, actual, mark)
                api.absent(stage.handle, private)
                self._parents(held)
            return {"container": container, "node": node, "bytes": length, "sha256": sha256,
                    "new_file": final_name, "origin_recorded": mark is not None}
        except Exception as error:
            if may_be_published:
                raise Uncertain(f"无法确认原目的文件：{final_name!r}。完整副本可能已经出现；"
                                "请先检查原位置，不自动重试或删除。") from error
            if staged:
                message = (f"{error}\n未完成正式发布；私用暂存名 {stage_name!r} 可能保留数据。"
                           "不自动重试或清理。")
                raise (CancelledCopy if isinstance(error, CancelledCopy) else Rejected)(message) from error
            raise

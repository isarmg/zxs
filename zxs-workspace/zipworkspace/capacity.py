"""Annotate actual capacity errors without changing their type, retry rules or original errno."""
from contextlib import contextmanager
import errno
import os
import sqlite3

_PREFIX = "容量诊断："


def observed_available(fd):
    """Advisory only: usable bytes the filesystem reports for this held object now."""
    try:
        v = os.fstatvfs(fd)
        unit, blocks, free, usable = v.f_frsize, v.f_blocks, v.f_bfree, v.f_bavail
    except (OSError, AttributeError):
        return None
    if (all(type(x) is int for x in (unit, blocks, free, usable))
            and unit > 0 and blocks > 0 and 0 <= usable <= free <= blocks):
        return unit * usable
    return None


def note(error, role, path, stage, operation=None, fd=None, *, database=False):
    if database:
        code = getattr(error, "sqlite_errorcode", None)
        if not isinstance(error, sqlite3.Error) or not isinstance(code, int) or code & 255 != sqlite3.SQLITE_FULL:
            return
        reason = "SQLITE_FULL：数据库或其临时存储无法继续扩展；原因未定，不能断言该卷已满或配额已尽"
    elif isinstance(error, OSError) and error.errno == errno.ENOSPC:
        reason = "ENOSPC：系统报告没有可分配空间或目录资源"
    elif isinstance(error, OSError) and hasattr(errno, "EDQUOT") and error.errno == errno.EDQUOT:
        reason = "EDQUOT：系统报告配额用尽"
    else:
        return
    if any(n.startswith(_PREFIX) for n in getattr(error,"__notes__",())): return
    where = str(path)
    if fd is not None:
        try: where += f"（原目录设备号 {os.fstat(fd).st_dev}）"
        except OSError: pass  # Never reopen a path during failure reporting.
    if callable(operation): operation = operation()  # An ID genuinely assigned by this native transaction, if any.
    op = f"；原操作号 {operation}" if operation else ""
    error.add_note(f"{_PREFIX}{role} {where}；{stage}；{reason}{op}。"
                   "保留已有候选或暂存；操作号不等于已受理或已完成，不清理、不另发，请查询原 STATE 回执。")


@contextmanager
def writing(role, path, stage, operation=None, fd=None):
    try: yield
    except OSError as error:
        note(error,role,path,stage,operation,fd)
        raise


@contextmanager
def buffered_output(stream, role, path, operation, parent_fd):
    """A failed buffered close can be the first (or last) reported write error."""
    try: yield stream
    finally:
        with writing(role, path, "关闭并刷新原写入句柄", operation, parent_fd):
            stream.close()


def close_output_fd(fd, role, path, operation, parent_fd):
    # close(2) may report a delayed write error after releasing the descriptor.
    # Never retry it: the descriptor number may already have been reused.
    with writing(role, path, "关闭原可写文件描述符时收到延迟写入错误", operation, parent_fd):
        os.close(fd)


def describe(error):
    """CLI and Qt show outer notes and relevant capacity details from an explicitly chained cause."""
    lines = [str(error), *getattr(error,"__notes__",())]
    cause = getattr(error,"__cause__",None)
    seen = {id(error)}
    while cause is not None and id(cause) not in seen:
        seen.add(id(cause))
        for item in getattr(cause,"__notes__",()):
            if item.startswith(_PREFIX) and item not in lines: lines.append(item)
        cause = getattr(cause,"__cause__",None)
    return "\n".join(lines)

/* Optional CPython 3.12+ qualification module. macFUSE 5.5.0 Library-2 headers.
 * No fuse_mount, fuse_main, Darwin channel, device FD, event loop, helper or volume.
 * The caller separately verifies the installed binary, interpreter and this module.
 */
#define PY_SSIZE_T_CLEAN
#define _FILE_OFFSET_BITS 64
#define FUSE_USE_VERSION 29
#include <Python.h>
#include <fuse.h>
#include <fuse_lowlevel.h>
#include <fuse_kernel.h>
#include <sys/uio.h>
#include <sys/stat.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#if !defined(__APPLE__) || !defined(__LP64__)
#error "Darwin LP64 only"
#endif

struct owner {
    PyObject *hooks, *error;   /* All callbacks here are synchronous on the owner with the GIL. */
    uint64_t serial;
    unsigned int replies, destroyed, reads, releases;
    int bad;
    size_t n;
    unsigned char body[1024];
};

static struct owner *current(void) {
    return (struct owner *)fuse_get_context()->private_data;
}

static int py_failed(struct owner *o) {
    if (!o->error) o->error = PyErr_GetRaisedException();
    else PyErr_Clear();
    o->bad = 1;
    return -EIO;
}

static int attrs(const char *path, struct stat *s) {
    struct owner *o = current();
    unsigned long long ino, size;
    unsigned int mode, nlink, uid, gid;
    PyObject *result = PyObject_CallMethod(o->hooks, "attr", "s", path);
    if (!result) return py_failed(o);
    int ok = PyArg_ParseTuple(result, "KKIIII", &ino, &size, &mode, &nlink, &uid, &gid);
    Py_DECREF(result);
    if (!ok) return py_failed(o);
    if (!ino || size > INT64_MAX || mode > UINT16_MAX || !nlink || nlink > UINT16_MAX) return -EIO;
    memset(s, 0, sizeof(*s));
    s->st_ino = ino; s->st_size = size; s->st_mode = mode; s->st_nlink = nlink;
    s->st_uid = uid; s->st_gid = gid;
    return 0;
}

static int open_member(const char *path, struct fuse_file_info *fi) {
    struct owner *o = current();
    PyObject *result = PyObject_CallMethod(o->hooks, "open", "si", path, fi->flags);
    if (!result) return py_failed(o);
    fi->fh = PyLong_AsUnsignedLongLong(result);
    Py_DECREF(result);
    if (PyErr_Occurred()) return py_failed(o);
    fi->direct_io = 1; fi->keep_cache = 0;
    return 0;
}

static int read_member(const char *path, char *buf, size_t n, off_t at, struct fuse_file_info *fi) {
    struct owner *o = current();
    if (n > 1024 || at < 0) return -EINVAL;
    PyObject *result = PyObject_CallMethod(o->hooks, "read", "snLK", path, (Py_ssize_t)n,
                                          (long long)at, (unsigned long long)fi->fh);
    if (!result) return py_failed(o);
    if (!PyBytes_CheckExact(result) || PyBytes_GET_SIZE(result) > (Py_ssize_t)n) {
        Py_DECREF(result); return -EIO;
    }
    int got = (int)PyBytes_GET_SIZE(result);
    memcpy(buf, PyBytes_AS_STRING(result), (size_t)got);
    Py_DECREF(result); o->reads++;
    return got;
}

static int release_member(const char *path, struct fuse_file_info *fi) {
    struct owner *o = current();
    PyObject *result = PyObject_CallMethod(o->hooks, "release", "sK", path, (unsigned long long)fi->fh);
    if (!result) return py_failed(o);
    Py_DECREF(result); o->releases++;
    return 0;
}

static const struct fuse_operations operations = {
    .getattr = attrs, .open = open_member, .read = read_member, .release = release_member
};

static int no_receive(struct fuse_chan **ch, char *buf, size_t n) {
    (void)ch; (void)buf; (void)n;
    return -ENODEV; /* There is never an OS transport to read or wait for. */
}

static void closed(struct fuse_chan *ch) {
    struct owner *o = fuse_chan_data(ch);
    o->destroyed++;
}

static int reply(struct fuse_chan *ch, const struct iovec *iov, size_t count) {
    struct owner *o = fuse_chan_data(ch);
    struct fuse_out_header h;
    /* Each fixed input gets one response; copy a bounded result, never expose a raw address. */
    if (o->replies++ || count < 1 || count > 8 || iov[0].iov_len != sizeof(h)) goto fail;
    memcpy(&h, iov[0].iov_base, sizeof(h));
    if (h.unique != o->serial || h.error || h.len < sizeof(h) || h.len - sizeof(h) > sizeof(o->body)) goto fail;
    for (size_t i = 1; i < count; i++) {
        if (iov[i].iov_len > sizeof(o->body) - o->n) goto fail;
        if (iov[i].iov_len) memcpy(o->body + o->n, iov[i].iov_base, iov[i].iov_len);
        o->n += iov[i].iov_len;
    }
    if (o->n == h.len - sizeof(h)) return 0;
fail:
    o->bad = 1;
    return -EIO;
}

struct frame {
    struct fuse_in_header h;
    union { struct fuse_init_in init; char name[10]; struct fuse_open_in open;
            struct fuse_read_in read; struct fuse_release_in release; } in;
};
_Static_assert(offsetof(struct frame, in) == sizeof(struct fuse_in_header), "SDK request alignment");

static int original(struct fuse *f, struct fuse_chan *ch, struct owner *o) {
    if (o->destroyed) return 0;
    struct fuse_session *se = fuse_get_session(f);
    return se && fuse_chan_session(ch) == se && fuse_session_next_chan(se, NULL) == ch &&
           fuse_chan_data(ch) == o && !fuse_session_exited(se);
}

static int admitted(struct owner *o) {
    if (o->bad || PyErr_CheckSignals() < 0) return 0;
    PyObject *ok = PyObject_CallMethod(o->hooks, "checkpoint", NULL);
    if (!ok) return 0;
    Py_DECREF(ok);
    return 1;
}

static int step(struct fuse *f, struct fuse_chan *ch, struct owner *o, struct frame *p,
                uint32_t opcode, uint64_t node, size_t n) {
    if (!original(f, ch, o)) return 0;
    p->h = (struct fuse_in_header){ .len=(uint32_t)(sizeof(p->h)+n), .opcode=opcode,
        .unique=++o->serial, .nodeid=node, .uid=getuid(), .gid=getgid(), .pid=(uint32_t)getpid() };
    o->n = 0; o->replies = 0;
    fuse_session_process(fuse_get_session(f), (const char *)p, p->h.len, ch);
    return o->replies == 1 && !o->bad && original(f, ch, o);
}

/* Check the linked function's actual image; a matching basename/version is not sufficient.
 * Does not authenticate preinstalled software: this extension and the SDK must already be trusted. */
static int matches_install(void) {
    Dl_info image;
    struct stat seen, expected;
    return dladdr((const void *)&fuse_new, &image) && image.dli_fname &&
           !stat(image.dli_fname, &seen) && !stat("/usr/local/lib/libfuse.2.dylib", &expected) &&
           S_ISREG(seen.st_mode) && seen.st_ino && seen.st_dev == expected.st_dev && seen.st_ino == expected.st_ino;
}

static PyObject *observe(PyObject *module, PyObject *hooks) {
    (void)module;
    char name[] = "zip-workspace-offline", options[] = "-ouse_ino,entry_timeout=0,attr_timeout=0,negative_timeout=0";
    char *argv[] = {name, options, NULL};
    struct fuse_args args = FUSE_ARGS_INIT(2, argv);
    struct fuse_chan_ops transport = {.receive=no_receive, .send=reply, .destroy=closed};
    struct owner o = {.hooks=hooks};
    struct fuse *f = NULL;
    struct frame p = {0};
    struct fuse_init_out init;
    struct fuse_entry_out entry;
    struct fuse_open_out opened;
    PyObject *prior = NULL;
    int read_ok = 0, have_open = 0, observed = 0;
    if (getuid() == 0 || getuid() != geteuid() || getgid() != getegid() || !matches_install() || fuse_version() != 29)
        return PyErr_Format(PyExc_RuntimeError, "requires ordinary user and approved macFUSE libfuse 2.9 ABI");
    if (!admitted(&o)) return NULL;
    struct fuse_chan *ch = fuse_chan_new(&transport, -1, 1 << 20, &o);
    if (!ch) return PyErr_NoMemory();
    f = fuse_new(ch, &args, &operations, sizeof(operations), &o);
    fuse_opt_free_args(&args);
    if (!f) goto done;
    /* 5.5 dispatches high-level destruction only after got_init. No Python call or
     * cancellation gate between successful construction and this fixed in-memory INIT. */
    p.in.init = (struct fuse_init_in){.major=FUSE_KERNEL_VERSION, .minor=FUSE_KERNEL_MINOR_VERSION};
    if (!step(f,ch,&o,&p,FUSE_INIT,0,sizeof(p.in.init)) || o.n != sizeof(init)) goto done;
    memcpy(&init,o.body,sizeof(init));
    if (init.major != FUSE_KERNEL_VERSION || init.minor != FUSE_KERNEL_MINOR_VERSION) goto done;
    memcpy(p.in.name,"probe.txt",sizeof(p.in.name));
    if (!admitted(&o) || !step(f,ch,&o,&p,FUSE_LOOKUP,FUSE_ROOT_ID,sizeof(p.in.name)) || o.n != sizeof(entry)) goto done;
    memcpy(&entry,o.body,sizeof(entry));
    if (!entry.nodeid || !S_ISREG(entry.attr.mode) || (entry.attr.mode & 0333) || entry.attr.size != 19) goto done;
    p.in.open = (struct fuse_open_in){.flags=O_RDONLY};
    if (!admitted(&o) || !step(f,ch,&o,&p,FUSE_OPEN,entry.nodeid,sizeof(p.in.open)) || o.n != sizeof(opened)) goto done;
    memcpy(&opened,o.body,sizeof(opened));
    have_open = 1;
    if (!opened.fh || (opened.open_flags & (FOPEN_DIRECT_IO | FOPEN_KEEP_CACHE)) != FOPEN_DIRECT_IO) goto done;
    p.in.read = (struct fuse_read_in){.fh=opened.fh, .offset=10, .size=8, .flags=O_RDONLY};
    if (!admitted(&o) || !step(f,ch,&o,&p,FUSE_READ,entry.nodeid,sizeof(p.in.read)) || o.reads != 1 ||
            o.n != 8 || memcmp(o.body,"snapshot",8)) goto done;
    read_ok = 1;
done:
    /* A pending exception cannot be left active across a distinct Python cleanup call. */
    prior = o.error; o.error = NULL;
    if (prior) PyErr_Clear(); else prior = PyErr_GetRaisedException();
    if (have_open) {
        p.in.release = (struct fuse_release_in){.fh=opened.fh, .flags=O_RDONLY};
        /* Once OPEN was accepted, send one original RELEASE even on cancellation or a
         * bad READ. This callback does not read the archive; it grants no new request. */
        int released = step(f,ch,&o,&p,FUSE_RELEASE,entry.nodeid,sizeof(p.in.release));
        observed = read_ok && released && o.releases == 1 && !o.n;
    }
    if (f) fuse_destroy(f);                        /* Synchronously disposes its own generic channel. */
    else if (!o.destroyed) fuse_chan_destroy(ch);  /* Late fuse_new failures already consumed it. */
    if (prior) { Py_XDECREF(o.error); PyErr_SetRaisedException(prior); return NULL; }
    if (o.error) { PyErr_SetRaisedException(o.error); return NULL; }
    if (PyErr_Occurred()) return NULL;
    if (!observed || o.destroyed != 1)
        return PyErr_Format(PyExc_RuntimeError, "unmounted session did not complete its original trace and teardown");
    Py_RETURN_TRUE;
}

static PyMethodDef methods[] = {
    {"observe", observe, METH_O, "One fixed in-process synthetic trace; never a mount."}, {NULL, NULL, 0, NULL}
};
static struct PyModuleDef definition = {PyModuleDef_HEAD_INIT, "_qualification_macos_session", NULL, -1, methods};
PyMODINIT_FUNC PyInit__qualification_macos_session(void) { return PyModule_Create(&definition); }

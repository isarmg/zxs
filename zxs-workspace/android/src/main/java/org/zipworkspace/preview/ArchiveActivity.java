package org.zipworkspace.preview;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.DialogInterface;
import android.content.Intent;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.text.InputFilter;
import android.text.InputType;
import android.text.TextUtils;
import android.view.View;
import android.view.inputmethod.EditorInfo;
import android.view.WindowInsets;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.TextView;
import android.widget.ScrollView;
import android.view.WindowManager;
import android.view.textclassifier.TextClassifier;
import java.io.IOException;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;

/** No ACTION_VIEW/deep links. Only a foreground user picker decision can supply a document. */
public final class ArchiveActivity extends Activity {
    private static final int PAGE = 100;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS,
            new ArrayBlockingQueue<>(1));
    private CancellationSignal active;
    private Runnable bodyProgress; // main thread owns the sole pending tick
    private PickedDocument document; // foreground-only original; never put in saved state
    private AlertDialog decision;
    private int generation, page;
    private int requested, nextRequest = 1;
    private TextView state, location;
    private Button pick, cancel, find, checkAll, up, prev, next;
    private LinearLayout listHost;
    private ListView list;
    private ArchiveIndex.Item folder;
    private NameSearch search;
    private TextPages textPages;

    private static final class TextPages {
        final PickedDocument picked;
        final ArchiveIndex.Item item;
        final int ticket;
        final AlertDialog dialog;
        final TextView body;
        final ScrollView area;
        final EditText jump;
        final Button jumpGo;
        PlainText.Preview checked;
        ArchiveIndex.TextIndex indexed;
        CancellationSignal reading;
        int page = -1;
        TextPages(PickedDocument picked, ArchiveIndex.Item item, int ticket, AlertDialog dialog, TextView body,
                  ScrollView area, PlainText.Preview checked, ArchiveIndex.TextIndex indexed, EditText jump, Button jumpGo) {
            this.picked = picked; this.item = item; this.ticket = ticket; this.dialog = dialog; this.body = body;
            this.area = area; this.checked = checked; this.indexed = indexed; this.jump = jump; this.jumpGo = jumpGo;
        }
        int pages() { return indexed != null ? indexed.pages() : checked == null ? 0 : checked.pages(); }
        void clear() {
            if (reading != null) { reading.cancel(); reading = null; }
            checked = null; indexed = null; body.setText("");
            if (jump != null) jump.setText("");
        }
    }

    private static final class NameSearch {
        final PickedDocument picked;
        List<ArchiveIndex.Match> matches; // null refers only to this pending search, not an empty result.
        int page;
        NameSearch(PickedDocument picked) { this.picked = picked; }
    }

    private static final class DisplayRow {
        final ArchiveIndex.Item item;
        final NameSearch request;
        final ArchiveIndex.Match match;
        final String title;
        DisplayRow(ArchiveIndex.Item item, NameSearch request, ArchiveIndex.Match match, String title) {
            this.item = item; this.request = request; this.match = match; this.title = title;
        }
        @Override public String toString() { return title; }
    }

    @Override public void onCreate(Bundle saved) {
        super.onCreate(saved);
        LinearLayout body = new LinearLayout(this);
        body.setOrientation(LinearLayout.VERTICAL);
        int gap = (int)(12 * getResources().getDisplayMetrics().density);
        body.setOnApplyWindowInsetsListener((v, win) -> {
            Insets i = win.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(i.left + gap, i.top + gap, i.right + gap, i.bottom + gap); return win;
        });
        TextView notice = new TextView(this);
        notice.setText("ZIP 目录预览\n打开目录只看声明。可另行核验成员、原包字节或 .zxs 整树；不解包、不认证来源。");
        body.addView(notice);
        pick = button(body, "选择本机 .zxs 或 Store ZIP", v -> choose());
        cancel = button(body, "取消本次操作", v -> {
            if (search != null && search.matches == null) search = null;
            stopJob();
            if (document == null) reset("已取消读取。");
            else { state.setText("已取消，没有新结果。"); render(); }
        });
        find = button(body, "按名称查找…", v -> askSearch());
        checkAll = button(body, "核验原归档…", v -> askArchive());
        state = summary(body, 6);
        location = summary(body, 3);
        LinearLayout nav = new LinearLayout(this); body.addView(nav);
        up = button(nav, "上一级", v -> goUp());
        prev = button(nav, "上一页", v -> movePage(-1));
        next = button(nav, "下一页", v -> movePage(1));
        listHost = body;
        setContentView(body);
        if (saved != null) {
            requested = saved.getInt("picker");
            nextRequest = saved.getInt("next", 1);
        }
        pick.setEnabled(requested == 0);
        reset(requested == 0 ? "请选择一份文件；不记住上次的选择。" : "等待原系统选择结果…");
    }
    private Button button(LinearLayout box, String title, View.OnClickListener listener) {
        Button b = new Button(this); b.setText(title); b.setOnClickListener(listener); box.addView(b); return b;
    }
    private TextView summary(LinearLayout parent, int lines) {
        TextView view = new TextView(this);
        view.setMaxLines(lines);
        view.setEllipsize(TextUtils.TruncateAt.END);
        parent.addView(view);
        return view;
    }

    private void replaceList(List<DisplayRow> rows, boolean enabled) {
        // AbsListView can deliver an earlier tap later using a position in
        // the *current* adapter. A semantic render needs a new ListView.
        ListView old = list;
        ListView current = new ListView(this);
        current.setAdapter(new ArrayAdapter<>(this, android.R.layout.simple_list_item_1, rows));
        current.setEnabled(enabled);
        current.setSaveEnabled(false);
        current.setOnItemClickListener((a, view, pos, id) -> {
            if (a != list || active != null || document == null || decision != null) return;
            Object shown = a.getItemAtPosition(pos);
            if (!(shown instanceof DisplayRow)) return;
            DisplayRow row = (DisplayRow)shown;
            if (row.match != null) locate(row.request, row.match);
            else if (search == null) {
                if (row.item.children != null) enter(row.item); else ask(document, row.item);
            }
        });
        list = current; // Reject the old instance before detaching it from the window.
        if (old != null) listHost.removeView(old);
        listHost.addView(current, new LinearLayout.LayoutParams(-1, 0, 1));
    }

    private void stopJob() {
        generation++;
        stopBodyProgress();
        if (active != null) active.cancel();
        active = null;
        worker.getQueue().clear(); // queued jobs don't own FDs; a running read owns its own lease
    }
    private void retire() {
        stopJob();
        search = null;
        if (textPages != null) { textPages.clear(); textPages = null; }
        AlertDialog old = decision; decision = null;
        if (old != null) old.dismiss();
        if (document != null) { document.close(); document = null; }
    }
    private boolean current(int ticket, CancellationSignal signal) {
        return ticket == generation && signal == active && !signal.isCanceled() && !isDestroyed();
    }
    private AtomicLong startBodyProgress(PickedDocument picked, int ticket, CancellationSignal signal,
                                         long total, String title, String measure, String details) {
        stopBodyProgress();
        AtomicLong count = new AtomicLong();
        bodyProgress = new Runnable() {
            private long displayed = -1;
            @Override public void run() {
                if (bodyProgress != this || !current(ticket, signal) || document != picked) return;
                long now = count.get();
                if (now != displayed) {
                    displayed = now;
                    state.setText(String.format(Locale.ROOT, "%s，可取消。\n已处理%s %,d / %,d B%s%s",
                            title, measure, now, total, now == total ? "；仍在核验。" : "。", details));
                }
                main.postDelayed(this, 350); // the worker never posts per-block UI updates
            }
        };
        bodyProgress.run();
        return count;
    }
    private void stopBodyProgress() {
        Runnable old = bodyProgress; bodyProgress = null;
        if (old != null) main.removeCallbacks(old);
    }
    private void submit(Runnable task) {
        try { worker.execute(task); }
        catch (RejectedExecutionException e) { retire(); reset("本次读取未开始，请重新选择。"); }
    }
    private void reset(String message) {
        folder = null; page = 0; search = null; replaceList(List.of(), false);
        state.setText(message); location.setText("");
        checkAll.setVisibility(View.GONE);
        find.setEnabled(false); up.setEnabled(false); prev.setEnabled(false); next.setEnabled(false);
        cancel.setVisibility(active == null ? View.GONE : View.VISIBLE);
    }
    private void choose() {
        if (requested != 0) return;
        if (nextRequest > 65535) { retire(); reset("请关闭并重开窗口，再选择文件。"); return; }
        retire(); reset("等待系统文件选择器…");
        requested = nextRequest++; pick.setEnabled(false);
        try { startActivityForResult(PickedDocument.picker(), requested); }
        catch (ActivityNotFoundException | SecurityException e) {
            requested = 0; pick.setEnabled(true); reset("系统文件选择器不可用。");
        }
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (requested == 0 || request != requested) return;
        requested = 0; pick.setEnabled(true);
        if (result != RESULT_OK) { reset("没有选择文件。"); return; }
        final Uri uri;
        try { uri = PickedDocument.single(data); }
        catch (IOException | RuntimeException e) { reset("未获得支持的单一本机文件读取授权。"); return; }
        CancellationSignal signal = new CancellationSignal();
        int ticket = ++generation; active = signal; reset("正在读取目录声明…");
        submit(() -> {
            PickedDocument read = null;
            try { read = PickedDocument.inspect(getApplicationContext(), uri, signal); }
            catch (IOException | RuntimeException ignored) { /* no names/URIs in logs or error text */ }
            final PickedDocument resultView = read;
            // Cancellation also owns a just-opened result until the main thread adopts it.
            if (resultView != null) signal.setOnCancelListener(resultView::close);
            boolean posted = main.post(() -> {
                signal.setOnCancelListener(null);
                if (!current(ticket, signal)) { if (resultView != null) resultView.close(); return; }
                active = null; reset("");
                if (resultView == null) {
                    reset("无法预览。请选择主共享存储中的完整 Store ZIP；权限、文件变化、格式或大小限制也会导致拒绝。");
                } else {
                    document = resultView;
                    state.setText(document.title + "（目录声明）"); enter(document.index.root);
                }
            });
            if (!posted && resultView != null) resultView.close();
        });
    }
    private static void appendPath(StringBuilder path, ArchiveIndex.Item row) {
        if (row.parent != null) {
            appendPath(path, row.parent);
            path.append('/').append(row.name);
        }
    }
    private static String pathOf(ArchiveIndex.Item row) {
        StringBuilder path = new StringBuilder();
        appendPath(path, row);
        return path.length() == 0 ? "/" : path.toString();
    }

    private void askSearch() {
        PickedDocument picked = document;
        if (picked == null || folder == null || active != null || decision != null) return;
        EditText query = new EditText(this);
        query.setSingleLine(true);
        query.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        query.setFilters(new InputFilter[]{new InputFilter.LengthFilter(ArchiveIndex.MAX_QUERY)});
        query.setImeOptions(EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        query.setHint("输入部分名称，区分大小写");
        query.setSaveEnabled(false);
        query.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
        query.setImportantForContentCapture(View.IMPORTANT_FOR_CONTENT_CAPTURE_NO_EXCLUDE_DESCENDANTS);
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("查找文件或目录名称")
                .setMessage("只在本次已读取的归档目录记录中查找，不读取正文。")
                .setView(query).setNegativeButton("取消", null)
                .setPositiveButton("查找", (d, which) -> startSearch(picked, query.getText().toString()))
                .create();
        showDecision(dialog, query); // Discard the draft on every dismissal.
    }

    private void startSearch(PickedDocument picked, String query) {
        if (document != picked || active != null || folder == null) return;
        if (query.isEmpty()) { state.setText("请先输入要查找的名称片段。"); return; }
        NameSearch request = new NameSearch(picked);
        CancellationSignal signal = new CancellationSignal();
        int ticket = ++generation;
        search = request; active = signal;
        state.setText(picked.title + "（仅在目录声明中查找）");
        render();
        try {
            worker.execute(() -> {
                List<ArchiveIndex.Match> read = null;
                try { read = picked.index.findNames(query, signal::throwIfCanceled); }
                catch (RuntimeException ignored) { /* no search terms or paths in failure text */ }
                final List<ArchiveIndex.Match> matches = read;
                main.post(() -> finishSearch(request, ticket, signal, matches));
            });
        } catch (RejectedExecutionException unavailable) {
            // A failed in-memory search is not a file/provider failure.
            finishSearch(request, ticket, signal, null);
        }
    }

    private void finishSearch(NameSearch request, int ticket, CancellationSignal signal,
                              List<ArchiveIndex.Match> matches) {
        if (search != request || document != request.picked || !current(ticket, signal)) return;
        active = null;
        if (matches == null) {
            search = null;
            state.setText("未完成名称查找；仍可浏览原目录。");
        } else {
            request.matches = matches;
        }
        render();
    }

    private void locate(NameSearch request, ArchiveIndex.Match match) {
        if (request == null || request != search || document != request.picked || request.matches == null
                || match.item.parent == null || !document.index.owns(match.item)) return;
        folder = match.item.parent;  // A real index node: no lookup by the displayed path.
        page = match.ordinal / PAGE;
        search = null;
        state.setText("已定位到所选条目所在页；未打开成员。");
        render();
        list.setSelection(match.ordinal % PAGE);
    }

    private void goUp() {
        if (active != null || folder == null) return;
        if (search != null) {
            search = null;
            state.setText(document.title + "（目录声明）");
            render(); // folder and its page were untouched by searching
        } else enter(folder.parent);
    }

    private void movePage(int delta) {
        if (active != null || folder == null) return;
        if (search == null) page += delta; else search.page += delta;
        render();
    }
    private void askArchive() {
        PickedDocument picked = document;
        if (picked == null || folder == null || active != null
                || search != null || decision != null) return;
        ArchiveIndex.SnapshotMarker marker = null;
        if (picked.isManagedSnapshotName()) {
            try { marker = picked.index.snapshotMarker(); }
            catch (IOException badMarker) { /* a malformed marker can never authorize the logical read */ }
        }
        int approvedTicket = generation;
        String raw = String.format(Locale.ROOT, "原包 SHA-256：读取原 ZIP 完整的 %,d 字节，包含头及注释。", picked.fileBytes());
        String logical = marker != null ? String.format(Locale.ROOT,
                "\n.zxs 整树：读取全部成员正文 %,d 字节并复核布局。节点 %s。", picked.index.bodyBytes(), marker.operation)
                : picked.isManagedSnapshotName() ? "\n没有可识别的规范快照标记，不提供整树核验。" : "";
        AlertDialog.Builder builder = new AlertDialog.Builder(this).setTitle("只读核验本次原归档")
                .setMessage(picked.title + "\n" + raw + logical + "\n用时取决于大小，可取消。不认证来源；不保存或上传。")
                .setNegativeButton("返回目录", null);
        if (marker != null) builder
                .setPositiveButton("核验 .zxs 整树", (d, which) -> startArchive(d, approvedTicket, picked, false))
                .setNeutralButton("核对原包 SHA-256", (d, which) -> startArchive(d, approvedTicket, picked, true));
        else builder.setPositiveButton("核对原包 SHA-256", (d, which) -> startArchive(d, approvedTicket, picked, true));
        showDecision(builder.create(), null);
    }

    private void startArchive(DialogInterface approved, int approvedTicket, PickedDocument picked, boolean raw) {
        if (decision == null || decision != approved || !decision.isShowing() || generation != approvedTicket
                || document != picked || active != null || search != null || folder == null) return;
        AlertDialog used = decision; decision = null; used.dismiss();
        CancellationSignal signal = new CancellationSignal();
        int ticket = ++generation; active = signal;
        AtomicLong bytes = startBodyProgress(picked, ticket, signal, raw ? picked.fileBytes() : picked.index.bodyBytes(),
                raw ? "正在核对原包 SHA-256" : "正在核验 .zxs 整树", raw ? "原包字节" : "正文", "");
        render();
        submit(() -> {
            String result = null;
            try {
                if (raw) result = picked.rawDigest(getApplicationContext(), signal, bytes::addAndGet);
                else {
                    ArchiveIndex.SnapshotMarker marker = picked.verifySnapshot(getApplicationContext(), signal, bytes::addAndGet);
                    result = String.format(Locale.ROOT,
                            "整树内容与包内标记一致。未认证来源。\n第 %d 代 · 节点 %s\n已核 %,d 项、%,d B 正文。",
                            marker.generation, marker.operation, picked.index.entryCount(), picked.index.bodyBytes());
                }
            }
            catch (IOException | RuntimeException ignored) { /* no URI, partial digest, or stale progress */ }
            String completed = result;
            main.post(() -> {
                if (!current(ticket, signal) || document != picked) return;
                stopBodyProgress();
                active = null;
                if (completed == null) {
                    retire(); reset(raw ? "没有取得原包摘要：字节不完整、原件变化或授权失效。请重新选择。"
                            : "没有通过整树核验：布局不符、读取不完整、内容不符或原授权失效。请重新选择。");
                    return;
                }
                try { picked.checkPresentation(getApplicationContext()); }
                catch (IOException | RuntimeException stale) {
                    retire(); reset("原文件状态或授权已失效，没有保留核验结果。");
                    return;
                }
                state.setText(raw ? "已读取原包完整字节；摘要不认证来源。" : completed);
                render();
                if (raw) showRaw(picked, completed);
            });
        });
    }

    private void showRaw(PickedDocument picked, String digest) {
        ScrollView area = new ScrollView(this);
        TextView value = new TextView(this);
        value.setTextIsSelectable(true); value.setTextClassifier(TextClassifier.NO_OP); value.setSaveEnabled(false);
        value.setText(String.format(Locale.ROOT, "%s\n完整 ZIP：%,d B\n\n%s\n\n包含头、成员与注释，不认证来源。",
                picked.title, picked.fileBytes(), digest));
        area.addView(value);
        showDecision(new AlertDialog.Builder(this).setTitle("原包 SHA-256").setView(area)
                .setPositiveButton("关闭", null).create(), value);
    }

    private void ask(PickedDocument picked, ArchiveIndex.Item row) {
        if (decision != null) return;
        int approvedTicket = generation;
        String total = String.format(Locale.ROOT, "%,d", row.bytes);
        String reading = "所有选项都先读取本项全部 " + total + " 字节，较大文件可能耗时，可以取消。";
        String source = "\n不保存、不上传。\n" + picked.title + "\n" + pathOf(row);
        if (row.bytes <= PlainText.LIMIT) {
            AlertDialog dialog = new AlertDialog.Builder(this)
                    .setTitle("查看这个归档成员")
                    .setMessage(reading + "\n仅当 CRC 相符且全文符合 UTF-8 显示规则时才显示全文。" + source)
                    .setNegativeButton("取消", null)
                    .setPositiveButton("只核验 CRC",
                            (d, which) -> startIfCurrent(d, approvedTicket, picked, row, StoredMember.Mode.CRC))
                    .setNeutralButton("核验后看全文",
                            (d, which) -> startIfCurrent(d, approvedTicket, picked, row, StoredMember.Mode.PREVIEW))
                    .create();
            showDecision(dialog, null);
            return;
        }
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        int pad = (int)(16 * getResources().getDisplayMetrics().density);
        box.setPadding(pad, pad, pad, pad);
        TextView detail = new TextView(this);
        detail.setText(reading + "\n按页跳读要求全文符合 UTF-8 显示规则；前部仅判断开头最多 256 KiB。" + source);
        box.addView(detail);
        RadioGroup choices = new RadioGroup(this);
        choices.setOrientation(RadioGroup.VERTICAL);
        int crc = addChoice(choices, "只核验 CRC");
        int prefix = addChoice(choices, "完整核验后预览前部");
        int indexed = addChoice(choices, "完整核验后按页跳读全文");
        box.addView(choices);
        ScrollView area = new ScrollView(this); area.addView(box);
        AlertDialog dialog = new AlertDialog.Builder(this).setTitle("选择如何查看这个成员")
                .setView(area).setNegativeButton("取消", null).setPositiveButton("开始", null).create();
        showDecision(dialog, null);
        Button go = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        go.setEnabled(false);
        choices.setOnCheckedChangeListener((group, id) -> go.setEnabled(id == crc || id == prefix || id == indexed));
        go.setOnClickListener(v -> {
            int id = choices.getCheckedRadioButtonId();
            if (id == crc || id == prefix || id == indexed) {
                StoredMember.Mode mode = id == crc ? StoredMember.Mode.CRC
                        : id == prefix ? StoredMember.Mode.PREVIEW : StoredMember.Mode.INDEXED;
                startIfCurrent(dialog, approvedTicket, picked, row, mode);
            }
        });
    }
    private int addChoice(RadioGroup group, String title) {
        RadioButton choice = new RadioButton(this);
        choice.setId(View.generateViewId()); choice.setText(title); group.addView(choice);
        return choice.getId();
    }
    private void showDecision(AlertDialog dialog, TextView body) {
        decision = dialog;
        dialog.setOnDismissListener(d -> {
            if (body != null) body.setText("");
            if (decision == dialog) decision = null;
        });
        dialog.show();
    }
    private void showText(PickedDocument picked, ArchiveIndex.Item row, int ticket, StoredMember.Result result) {
        PlainText.Preview text = result.text;
        boolean indexed = result.indexed != null;
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL); box.setSaveEnabled(false);
        TextView metadata = summary(box, 5);
        String hint = indexed ? "可直接输入页码；每页只回读并复核所需块，不再次扫描全文。"
                : text.complete() ? "显示全文。" : String.format(Locale.ROOT,
                        "只显示前 %,d 字节，不判定后文是否为文本。", text.displayedBytes);
        metadata.setText(String.format(Locale.ROOT, "已核验本项全部 %,d 字节。%s\n%s", row.bytes, hint, pathOf(row)));
        EditText jump = indexed ? new EditText(this) : null;
        Button jumpGo = null;
        if (jump != null) {
            jump.setSingleLine(true);
            jump.setInputType(InputType.TYPE_CLASS_NUMBER);
            jump.setFilters(new InputFilter[]{new InputFilter.LengthFilter(9)});
            jump.setImeOptions(EditorInfo.IME_ACTION_GO | EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
            jump.setHint("页码 1–" + result.indexed.pages());
            jump.setSaveEnabled(false);
            jump.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
            jump.setImportantForContentCapture(View.IMPORTANT_FOR_CONTENT_CAPTURE_NO_EXCLUDE_DESCENDANTS);
            box.addView(jump);
            jumpGo = button(box, "跳到该页", null);
        }
        TextView body = new TextView(this);
        body.setAutoLinkMask(0); body.setLinksClickable(false); body.setTextIsSelectable(false);
        body.setTextClassifier(TextClassifier.NO_OP); body.setSaveEnabled(false);
        body.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
        body.setImportantForContentCapture(View.IMPORTANT_FOR_CONTENT_CAPTURE_NO_EXCLUDE_DESCENDANTS);
        int pad = (int)(16 * getResources().getDisplayMetrics().density);
        box.setPadding(pad, pad, pad, pad);
        box.addView(body);
        ScrollView area = new ScrollView(this); area.setSaveEnabled(false); area.addView(box);
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle(indexed || text.complete() ? "核验后的全文" : "核验后仅看前部")
                .setView(area).setCancelable(false).setNegativeButton("关闭", null)
                .setNeutralButton("上一页", null).setPositiveButton("下一页", null).create();
        TextPages opened = new TextPages(picked, row, ticket, dialog, body, area, text, result.indexed, jump, jumpGo);
        textPages = opened; decision = dialog;
        dialog.setOnDismissListener(d -> {
            opened.clear();
            if (textPages == opened) textPages = null;
            if (decision == dialog) decision = null;
        });
        dialog.getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN);
        dialog.show();
        dialog.getButton(DialogInterface.BUTTON_NEGATIVE).setOnClickListener(v -> {
            if (textPages != opened || decision != dialog) return;
            opened.clear(); textPages = null; decision = null;
            dialog.dismiss();
        });
        dialog.getButton(DialogInterface.BUTTON_NEUTRAL).setOnClickListener(v -> showTextPage(opened, opened.page - 1));
        dialog.getButton(DialogInterface.BUTTON_POSITIVE).setOnClickListener(v -> showTextPage(opened, opened.page + 1));
        if (jump != null) {
            jumpGo.setOnClickListener(v -> jumpToPage(opened));
            jump.setOnEditorActionListener((view, action, event) -> {
                if (action != EditorInfo.IME_ACTION_GO) return false;
                jumpToPage(opened); return true;
            });
        }
        showTextPage(opened, 0);
    }
    private boolean currentText(TextPages shown) {
        return textPages == shown && decision == shown.dialog && shown.dialog.isShowing()
                && generation == shown.ticket && document == shown.picked && active == null
                && folder == shown.item.parent && search == null && !isDestroyed();
    }
    private boolean checkTextGrant(TextPages shown) {
        try { shown.picked.checkPresentation(getApplicationContext()); return true; }
        catch (IOException | RuntimeException lost) {
            retire(); reset("原文件已变化或授权不可用，已关闭文本。请重新选择。");
            return false;
        }
    }
    private void jumpToPage(TextPages shown) {
        if (!currentText(shown) || shown.indexed == null || shown.reading != null) return;
        final int number;
        try { number = Integer.parseInt(shown.jump.getText().toString()); }
        catch (NumberFormatException invalid) { shown.jump.setError("请输入有效页码。"); return; }
        if (number < 1 || number > shown.pages()) { shown.jump.setError("页码不在本项范围内。"); return; }
        shown.jump.setText(""); shown.jump.clearFocus();
        showTextPage(shown, number - 1);
    }
    private void textButtons(TextPages shown, boolean ready) {
        shown.dialog.getButton(DialogInterface.BUTTON_NEUTRAL).setEnabled(ready && shown.page > 0);
        shown.dialog.getButton(DialogInterface.BUTTON_POSITIVE).setEnabled(ready && shown.page + 1 < shown.pages());
        if (shown.jump != null) {
            shown.jump.setEnabled(ready); shown.jumpGo.setEnabled(ready);
        }
    }
    private void drawTextPage(TextPages shown, int page, String body) {
        if (!currentText(shown) || !checkTextGrant(shown)) return;
        shown.page = page;
        PlainText.Preview text = shown.checked;
        String kind = text == null ? "核验后按页跳读"
                : text.complete() ? (text.totalBytes == 0 ? "空文本" : "核验后的全文") : "核验后仅看前部";
        shown.dialog.setTitle(kind + " · " + (page + 1) + "/" + shown.pages() + " 页");
        shown.body.setText(body, TextView.BufferType.NORMAL); // never links or markup
        shown.area.scrollTo(0, 0);
        textButtons(shown, true);
    }
    private void showTextPage(TextPages shown, int page) {
        if (!currentText(shown) || shown.reading != null || page < 0 || page >= shown.pages()) return;
        if (shown.checked != null) { drawTextPage(shown, page, shown.checked.page(page)); return; }
        if (!checkTextGrant(shown)) return;
        ArchiveIndex.TextIndex proof = shown.indexed;
        CancellationSignal signal = new CancellationSignal();
        shown.reading = signal;
        shown.body.setText("");
        shown.dialog.setTitle("正在读取第 " + (page + 1) + " 页，可关闭取消");
        textButtons(shown, false);
        submit(() -> {
            String read = null;
            try { read = shown.picked.textPage(getApplicationContext(), shown.item, proof, page, signal); }
            catch (IOException | RuntimeException ignored) { /* never show a partial or stale page */ }
            final String result = read;
            main.post(() -> {
                if (!currentText(shown) || shown.reading != signal || signal.isCanceled()) return;
                shown.reading = null;
                if (result == null) {
                    retire(); reset("无法复核这一页的原件、授权或字节，已关闭文本。请重新选择。");
                } else drawTextPage(shown, page, result);
            });
        });
    }
    private void startIfCurrent(DialogInterface approved, int approvedTicket, PickedDocument picked,
                                ArchiveIndex.Item row, StoredMember.Mode readMode) {
        if (decision == null || decision != approved || !decision.isShowing() || generation != approvedTicket
                || document != picked || active != null || search != null || folder != row.parent) return;
        AlertDialog used = decision; decision = null; used.dismiss();
        CancellationSignal signal = new CancellationSignal();
        int ticket = ++generation; active = signal;
        String path = pathOf(row);
        String mode = readMode == StoredMember.Mode.CRC ? ""
                : readMode == StoredMember.Mode.INDEXED ? "\n同时核验全文与定位索引"
                : row.bytes <= PlainText.LIMIT ? "\nCRC 相符后显示全文" : "\nCRC 相符后只显示前部";
        AtomicLong bytes = startBodyProgress(picked, ticket, signal, row.bytes, "正在核验所选成员", "正文", mode + "\n" + path);
        render();
        submit(() -> {
            StoredMember.Result read = null;
            try { read = picked.checkMember(getApplicationContext(), row, signal, readMode, bytes::addAndGet); }
            catch (IOException | RuntimeException ignored) { /* don't disclose URIs or return a partial CRC */ }
            final StoredMember.Result result = read;
            main.post(() -> {
                if (!current(ticket, signal) || document != picked) return;
                stopBodyProgress();
                active = null;
                if (result == null) {
                    retire(); reset("未取得完整结果：成员布局不符、读取中断、文件已变或授权不可用。请重新选择。");
                } else {
                    boolean textAllowed = readMode != StoredMember.Mode.CRC && result.matches()
                            && folder == row.parent && search == null;
                    boolean canDisplay = result.text != null || result.indexed != null;
                    String verdict = (result.matches() ? "该项 CRC-32 与 ZIP 声明相符。" : "该项 CRC-32 与 ZIP 声明不符。")
                            + String.format(Locale.ROOT, "\n声明 %08X，实读 %08X。", result.expected, result.actual)
                            + "\n不是签名，也不证明整包或其他项。";
                    if (textAllowed && !canDisplay) {
                        verdict += readMode == StoredMember.Mode.INDEXED ? "\n全文不符合 UTF-8 文本显示规则，不提供跳读。"
                                : "\n本次预览范围不符合 UTF-8 文本显示规则，不显示正文。";
                    }
                    state.setText(verdict + "\n" + path + "（" + row.bytes + " B）\n" + picked.title);
                    render();
                    if (textAllowed && canDisplay) showText(picked, row, ticket, result);
                }
            });
        });
    }
    private void enter(ArchiveIndex.Item item) {
        if (item == null || item.children == null) return;
        search = null; folder = item; page = 0; render();
    }
    private static DisplayRow displayed(ArchiveIndex.Item item, NameSearch request, ArchiveIndex.Match match) {
        String name = match == null ? item.name : pathOf(item);
        return new DisplayRow(item, request, match,
                item.children != null ? "目录　" + name : name + "　" + item.bytes + " B");
    }

    private void render() {
        NameSearch request = search;
        List<ArchiveIndex.Match> matches = request == null || request.matches == null ? List.of() : request.matches;
        int count = request == null ? folder.children.size() : matches.size();
        int pages = Math.max(1, (count + PAGE - 1) / PAGE);
        int shownPage = Math.max(0, Math.min(request == null ? page : request.page, pages - 1));
        String bounds = (shownPage + 1) + "/" + pages + " 页";
        ArrayList<DisplayRow> rows = new ArrayList<>();
        int start = shownPage * PAGE;
        if (request != null) {
            request.page = shownPage;
            if (request.matches == null) location.setText("正在查找名称…");
            else if (count == 0) location.setText("没有匹配的文件或目录名称。");
            else location.setText("名称匹配 " + count + " 项 · " + bounds);
            for (int i = start; i < Math.min(start + PAGE, count); i++) {
                ArchiveIndex.Match match = matches.get(i);
                rows.add(displayed(match.item, request, match));
            }
        } else {
            page = shownPage;
            location.setText(pathOf(folder) + "   " + (count == 0 ? "空目录" : bounds));
            int i = 0;
            for (ArchiveIndex.Item item : folder.children.values()) {
                if (i++ < start) continue;
                if (rows.size() == PAGE) break;
                rows.add(displayed(item, null, null));
            }
        }
        boolean idle = active == null;
        replaceList(rows, idle);
        find.setEnabled(idle); cancel.setVisibility(idle ? View.GONE : View.VISIBLE);
        checkAll.setVisibility(document != null && request == null ? View.VISIBLE : View.GONE);
        checkAll.setEnabled(idle);
        up.setText(request == null ? "上一级" : "返回目录");
        up.setEnabled(idle && (request != null || folder.parent != null));
        prev.setEnabled(idle && shownPage > 0);
        next.setEnabled(idle && shownPage + 1 < pages);
    }
    @Override protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        out.putInt("picker", requested); out.putInt("next", nextRequest); // no document, grant or index
    }
    @Override protected void onStop() {
        super.onStop();
        if (requested == 0) { retire(); reset("已关闭本次文件；请重新选择。"); }
    }
    @Override protected void onDestroy() {
        retire(); worker.shutdownNow(); super.onDestroy(); // posted handoffs still close any unclaimed original
    }
}

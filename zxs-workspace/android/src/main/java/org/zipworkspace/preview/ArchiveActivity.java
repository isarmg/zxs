package org.zipworkspace.preview;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.graphics.Insets;
import android.net.Uri;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.view.WindowInsets;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.TextView;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Locale;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

/** No ACTION_VIEW/deep links. Only a foreground user picker decision can supply a document. */
public final class ArchiveActivity extends Activity {
    private static final int PAGE = 100;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1, 1, 0, TimeUnit.SECONDS,
            new ArrayBlockingQueue<>(1));
    private CancellationSignal active;
    private PickedDocument document; // foreground-only original; never put in saved state
    private AlertDialog decision;
    private int generation, page;
    private int requested, nextRequest = 1;
    private TextView state, location;
    private Button pick, cancel, up, prev, next;
    private ListView list;
    private ArchiveIndex.Item folder;
    private final ArrayList<ArchiveIndex.Item> visible = new ArrayList<>();

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
        notice.setText("ZIP 目录预览\n目录只显示声明。点普通文件可核验该项 CRC-32；不会解包，也不认证快照。");
        body.addView(notice);
        pick = button(body, "选择本机 .zxs 或 Store ZIP", v -> choose());
        cancel = button(body, "取消读取", v -> {
            stopJob();
            if (document == null) reset("已取消读取。");
            else { state.setText("已取消核验，没有结果。"); render(); }
        });
        state = new TextView(this); body.addView(state);
        location = new TextView(this); body.addView(location);
        LinearLayout nav = new LinearLayout(this); body.addView(nav);
        up = button(nav, "上一级", v -> enter(folder.parent));
        prev = button(nav, "上一页", v -> { page--; render(); });
        next = button(nav, "下一页", v -> { page++; render(); });
        list = new ListView(this);
        body.addView(list, new LinearLayout.LayoutParams(-1, 0, 1));
        list.setOnItemClickListener((a, v, pos, id) -> {
            if (active != null || document == null || pos < 0 || pos >= visible.size()) return;
            ArchiveIndex.Item row = visible.get(pos);
            if (row.children != null) enter(row); else ask(document, row);
        });
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
    private void stopJob() {
        generation++;
        if (active != null) active.cancel();
        active = null;
        worker.getQueue().clear(); // queued jobs don't own FDs; a running read owns its own lease
    }
    private void retire() {
        stopJob();
        if (decision != null) decision.dismiss();
        if (document != null) { document.close(); document = null; }
    }
    private boolean current(int ticket, CancellationSignal signal) {
        return ticket == generation && signal == active && !signal.isCanceled() && !isDestroyed();
    }
    private void submit(Runnable task) {
        try { worker.execute(task); }
        catch (RejectedExecutionException e) { retire(); reset("本次读取未开始，请重新选择。"); }
    }
    private void reset(String message) {
        folder = null; page = 0; visible.clear(); list.setAdapter(null);
        state.setText(message); location.setText("");
        up.setEnabled(false); prev.setEnabled(false); next.setEnabled(false);
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
    private static String pathOf(ArchiveIndex.Item row) {
        String path = "";
        for (; row.parent != null; row = row.parent) path = "/" + row.name + path;
        return path.isEmpty() ? "/" : path;
    }
    private void ask(PickedDocument picked, ArchiveIndex.Item row) {
        if (decision != null) return;
        AlertDialog dialog = new AlertDialog.Builder(this)
                .setTitle("核验这一项的 CRC-32？")
                .setMessage(picked.title + "\n" + pathOf(row) + "\n将读取全部 " + row.bytes
                        + " 字节，只与 ZIP 声明比较，不保存或上传。")
                .setNegativeButton("取消", null)
                .setPositiveButton("开始核验", (d, which) -> {
                    if (document == picked && active == null && folder == row.parent) verify(picked, row);
                }).create();
        decision = dialog;
        dialog.setOnDismissListener(d -> { if (decision == dialog) decision = null; });
        dialog.show();
    }
    private void verify(PickedDocument picked, ArchiveIndex.Item row) {
        CancellationSignal signal = new CancellationSignal();
        int ticket = ++generation; active = signal;
        String path = pathOf(row);
        state.setText("正在读取 " + path + "（" + row.bytes + " B）…"); render();
        submit(() -> {
            StoredMember.Result read = null;
            try { read = picked.checkMember(getApplicationContext(), row, signal); }
            catch (IOException | RuntimeException ignored) { /* don't disclose URIs or return a partial CRC */ }
            final StoredMember.Result result = read;
            main.post(() -> {
                if (!current(ticket, signal) || document != picked) return;
                active = null;
                if (result == null) {
                    retire(); reset("未取得完整结果：成员布局不符、读取中断、文件已变或授权不可用。请重新选择。");
                } else {
                    state.setText(picked.title + "\n" + path + "（" + row.bytes + " B）\n"
                            + (result.matches() ? "该项 CRC-32 与 ZIP 声明相符。" : "该项 CRC-32 与 ZIP 声明不符。")
                            + String.format(Locale.ROOT, "\n声明 %08X，实读 %08X。", result.expected, result.actual)
                            + " 不是签名，也不证明整包或其他项。");
                    render();
                }
            });
        });
    }
    private void enter(ArchiveIndex.Item item) {
        if (item == null || item.children == null) return;
        folder = item; page = 0; render();
    }
    private void render() {
        int count = folder.children.size(), pages = Math.max(1, (count + PAGE - 1) / PAGE);
        page = Math.max(0, Math.min(page, pages - 1));
        String path = pathOf(folder);
        location.setText(path + "   " + (count == 0 ? "空目录" : (page + 1) + "/" + pages + " 页"));
        visible.clear(); ArrayList<String> titles = new ArrayList<>();
        int i = 0;
        for (ArchiveIndex.Item row : folder.children.values()) {
            if (i++ < page * PAGE) continue;
            if (visible.size() == PAGE) break;
            visible.add(row);
            titles.add(row.children != null ? "目录　" + row.name : row.name + "　" + row.bytes + " B");
        }
        list.setAdapter(new ArrayAdapter<>(this, android.R.layout.simple_list_item_1, titles));
        boolean idle = active == null;
        list.setEnabled(idle); cancel.setVisibility(idle ? View.GONE : View.VISIBLE);
        up.setEnabled(idle && folder.parent != null); prev.setEnabled(idle && page > 0);
        next.setEnabled(idle && page + 1 < pages);
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

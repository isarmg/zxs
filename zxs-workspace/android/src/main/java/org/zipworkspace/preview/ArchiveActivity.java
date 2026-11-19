package org.zipworkspace.preview;

import android.app.Activity;
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
        notice.setText("ZIP 目录预览\n仅显示名称和声明大小；不打开成员、不校验正文、CRC 或快照身份。");
        body.addView(notice);
        pick = button(body, "选择本机 .zxs 或 Store ZIP", v -> choose());
        cancel = button(body, "取消读取", v -> { retire(); reset("已取消读取。"); });
        state = new TextView(this); body.addView(state);
        location = new TextView(this); body.addView(location);
        LinearLayout nav = new LinearLayout(this); body.addView(nav);
        up = button(nav, "上一级", v -> enter(folder.parent));
        prev = button(nav, "上一页", v -> { page--; render(); });
        next = button(nav, "下一页", v -> { page++; render(); });
        list = new ListView(this);
        body.addView(list, new LinearLayout.LayoutParams(-1, 0, 1));
        list.setOnItemClickListener((a, v, pos, id) -> {
            if (pos >= 0 && pos < visible.size() && visible.get(pos).children != null) enter(visible.get(pos));
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
    private void retire() {
        generation++;
        if (active != null) active.cancel();
        active = null;
        worker.getQueue().clear(); // no queued obsolete selections holding URI references
    }
    private void reset(String message) {
        folder = null; page = 0; visible.clear(); list.setAdapter(null);
        state.setText(message); location.setText("");
        up.setEnabled(false); prev.setEnabled(false); next.setEnabled(false);
        cancel.setVisibility(active == null ? View.GONE : View.VISIBLE);
    }
    private void choose() {
        if (requested != 0) return;
        if (nextRequest > 65535) { reset("请关闭并重开窗口，再选择文件。"); return; }
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
        worker.execute(() -> {
            PickedDocument read = null;
            try { read = PickedDocument.inspect(getApplicationContext(), uri, signal); }
            catch (IOException | RuntimeException ignored) { /* no names/URIs in logs or error text */ }
            final PickedDocument resultView = read;
            main.post(() -> {
                if (ticket != generation || signal != active || isDestroyed()) return;
                active = null; reset("");
                if (resultView == null) {
                    reset("无法预览。请选择主共享存储中的完整 Store ZIP；权限、文件变化、格式或大小限制也会导致拒绝。");
                } else {
                    state.setText(resultView.title + "（目录声明）"); enter(resultView.index.root);
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
        String path = "/";
        for (ArchiveIndex.Item n = folder; n.parent != null; n = n.parent) path = "/" + n.name + path;
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
        up.setEnabled(folder.parent != null); prev.setEnabled(page > 0); next.setEnabled(page + 1 < pages);
    }
    @Override protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        out.putInt("picker", requested); out.putInt("next", nextRequest); // no document, grant or index
    }
    @Override protected void onDestroy() {
        retire(); worker.shutdownNow(); main.removeCallbacksAndMessages(null); super.onDestroy();
    }
}

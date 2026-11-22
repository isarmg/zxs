package org.zipworkspace.preview;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Insets;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.InputFilter;
import android.text.InputType;
import android.view.WindowInsets;
import android.view.inputmethod.EditorInfo;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/** A fresh, single-text-file editor, separate from the read-only archive browser. */
public final class WorkspaceEditActivity extends Activity {
    private Session session;
    private EditText name, text;
    private Button commit;
    private TextView result;

    private static final class Session {
        String name = "", text = "", slot = "";
        boolean attempted;
        LocalWorkspaceStore.Receipt receipt;
        WorkspaceEditActivity screen;

        void begin(Context app, LocalWorkspaceDraft.Frozen snapshot) {
            if (attempted) return;
            attempted = true;
            slot = snapshot.filename();
            screen.showState();
            Handler main = new Handler(Looper.getMainLooper());
            new Thread(() -> {
                LocalWorkspaceStore.Receipt finished;
                try { finished = LocalWorkspaceStore.commit(app, snapshot); }
                catch (RuntimeException problem) {
                    finished = new LocalWorkspaceStore.Receipt(LocalWorkspaceStore.Status.UNCONFIRMED, slot);
                }
                LocalWorkspaceStore.Receipt delivered = finished;
                main.post(() -> {
                    receipt = delivered;
                    if (screen != null) screen.showState();
                });
            }, "workspace-commit").start();
        }
    }

    @Override protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        Object retained = getLastNonConfigurationInstance();
        session = retained instanceof Session s ? s : new Session();
        if (retained == null && saved != null && saved.getBoolean("attempted")) {
            session.attempted = true;
            session.slot = saved.getString("slot", "");
            session.receipt = new LocalWorkspaceStore.Receipt(LocalWorkspaceStore.Status.UNCONFIRMED, session.slot);
        }
        ScrollView scroll = new ScrollView(this);
        LinearLayout form = new LinearLayout(this);
        form.setOrientation(LinearLayout.VERTICAL);
        int gap = (int)(12 * getResources().getDisplayMetrics().density);
        form.setOnApplyWindowInsetsListener((view, win) -> {
            Insets bars = win.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            view.setPadding(bars.left + gap, bars.top + gap, bars.right + gap, bars.bottom + gap);
            return win;
        });
        scroll.addView(form);

        TextView title = new TextView(this);
        title.setText("本机工作区");
        title.setTextSize(22);
        title.setAccessibilityHeading(true);
        form.addView(title);

        name = new EditText(this);
        name.setHint("文件名");
        name.setSingleLine();
        name.setFilters(new InputFilter[]{new InputFilter.LengthFilter(240)});
        name.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        name.setImeOptions(EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        name.setSaveEnabled(false);
        name.setText(session.name);
        form.addView(name);

        text = new EditText(this);
        text.setHint("正文");
        text.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE);
        text.setImeOptions(EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        text.setMinLines(8);
        text.setFilters(new InputFilter[]{new InputFilter.LengthFilter(PlainText.LIMIT)});
        text.setSaveEnabled(false);
        text.setText(session.text);
        form.addView(text);

        commit = new Button(this);
        commit.setText("提交快照…");
        commit.setOnClickListener(v -> confirm());
        form.addView(commit);
        result = new TextView(this);
        result.setAccessibilityLiveRegion(android.view.View.ACCESSIBILITY_LIVE_REGION_POLITE);
        form.addView(result);
        setContentView(scroll);
        showState();
    }

    private void confirm() {
        if (session.attempted || isFinishing()) return;
        String filename = name.getText().toString();
        if (filename.contains("/")) {
            name.setError("请输入一个文件名。");
            return;
        }
        LocalWorkspaceDraft draft = new LocalWorkspaceDraft();
        try { draft.putText(filename, text.getText().toString()); }
        catch (IllegalArgumentException badEdit) {
            result.setText("请检查文件名、文本格式和长度。");
            return;
        }
        new AlertDialog.Builder(this).setTitle("提交到本机？")
                .setMessage(filename)
                .setNegativeButton("取消", null)
                .setPositiveButton("提交", (dialog, choice) -> {
                    if (!isFinishing() && !isDestroyed() && session.screen == this && !session.attempted)
                        session.begin(getApplicationContext(), draft.freeze());
                }).show();
    }

    private void showState() {
        boolean editable = !session.attempted;
        name.setEnabled(editable);
        text.setEnabled(editable);
        commit.setEnabled(editable);
        if (editable) return;
        if (session.receipt == null) {
            result.setText("正在提交…");
            return;
        }
        String filename = session.receipt.filename();
        result.setText(switch (session.receipt.status()) {
            case COMMITTED -> "已提交到本机。\n" + filename;
            case CONFLICT -> "快照已存在，本次未提交。";
            case NOT_COMMITTED -> "本次未提交。";
            case UNCONFIRMED -> filename.isEmpty() ? "提交结果未确认。" : "提交结果未确认。\n" + filename;
        });
    }

    @Override protected void onStart() {
        super.onStart();
        session.screen = this;
        showState();
    }

    @Override protected void onStop() {
        if (session.screen == this) session.screen = null;
        super.onStop();
    }

    @Override public Object onRetainNonConfigurationInstance() {
        session.name = name.getText().toString();
        session.text = text.getText().toString();
        return session; // In memory only, so rotating never starts a second write.
    }

    @Override protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        // Record only that a write was attempted, never the editable data.
        out.putBoolean("attempted", session.attempted);
        out.putString("slot", session.slot);
    }
}

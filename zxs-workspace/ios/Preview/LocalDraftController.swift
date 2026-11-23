import UIKit

final class LocalDraftController: UIViewController, UITextViewDelegate {
    private let store: LocalDraft
    private var original: LocalDraft.Revision?
    private let editor = UITextView()
    private let status = UILabel()
    private var editorBaseline: Data?
    private var canEdit = false

    init(store: LocalDraft) {
        self.store = store
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "本地草稿"
        view.backgroundColor = .systemBackground
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: "关闭", style: .plain,
            target: self, action: #selector(close))
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "保存草稿", style: .done,
            target: self, action: #selector(save))
        navigationItem.rightBarButtonItem?.isEnabled = false
        status.numberOfLines = 0
        status.font = .preferredFont(forTextStyle: .footnote)
        status.adjustsFontForContentSizeCategory = true
        editor.font = .preferredFont(forTextStyle: .body)
        editor.adjustsFontForContentSizeCategory = true
        editor.accessibilityLabel = "草稿正文"
        editor.delegate = self
        editor.autocorrectionType = .no
        editor.smartDashesType = .no
        editor.smartQuotesType = .no
        editor.smartInsertDeleteType = .no
        #if compiler(>=6.0)
        if #available(iOS 18.0, *) { editor.writingToolsBehavior = UIWritingToolsBehavior.none }
        #endif
        let stack = UIStackView(arrangedSubviews: [status, editor])
        stack.axis = .vertical
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        let safe = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: safe.topAnchor, constant: 8),
            stack.leadingAnchor.constraint(equalTo: safe.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: safe.trailingAnchor, constant: -12),
            stack.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor)
        ])
        do {
            original = try store.open()
            editor.text = original?.text ?? ""
            editorBaseline = Data(editor.text.utf8)
            canEdit = true
            status.text = original == nil ? "新建本地草稿" : "已打开本地草稿"
        } catch {
            editor.isEditable = false
            status.text = "无法打开本地草稿，未改动已保存的文件。"
        }
    }

    private var hasChanges: Bool {
        guard let editorBaseline else { return false }
        return Data(editor.text.utf8) != editorBaseline
    }

    func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
        guard canEdit, text.utf8.count <= LocalDraft.byteLimit,
              let old = textView.text, let bounds = Range(range, in: old) else { return false }
        let count = old.utf8.count - old[bounds].utf8.count + text.utf8.count
        if count > LocalDraft.byteLimit { status.text = "草稿最多 256 KiB。" }
        return count <= LocalDraft.byteLimit
    }

    func textViewDidChange(_ textView: UITextView) { updateSaveButton() }
    func textViewDidChangeSelection(_ textView: UITextView) { updateSaveButton() }

    private func updateSaveButton() {
        // Do not depend on an IME commit changing the visible bytes or giving
        // the delegate another event. The action checks composition itself.
        navigationItem.rightBarButtonItem?.isEnabled = canEdit && hasChanges
    }

    @objc private func save() {
        guard canEdit, hasChanges else { return }
        guard editor.markedTextRange == nil else {
            status.text = "请先完成输入法组词，再保存。"
            return
        }
        do {
            original = try store.save(editor.text, replacing: original)
            editorBaseline = Data(editor.text.utf8)
            navigationItem.rightBarButtonItem?.isEnabled = false
            status.text = "已保存本地草稿"
        } catch LocalDraft.Failure.tooLarge {
            status.text = "草稿超过 256 KiB，未保存。"
        } catch LocalDraft.Failure.changed {
            status.text = "已保存的草稿可能已变更。保留当前输入，请勿直接覆盖。"
        } catch {
            status.text = "未能确认保存结果，当前输入仍保留。"
        }
    }

    @objc private func close() {
        guard hasChanges || editor.markedTextRange != nil else { dismiss(animated: true); return }
        let alert = UIAlertController(title: "放弃未保存的更改？", message: nil, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "继续编辑", style: .cancel))
        alert.addAction(UIAlertAction(title: "放弃更改", style: .destructive) { [weak self] _ in
            self?.dismiss(animated: true)
        })
        present(alert, animated: true)
    }
}

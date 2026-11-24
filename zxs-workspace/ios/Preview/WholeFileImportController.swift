import Foundation
import UIKit
import UniformTypeIdentifiers

struct WholeFileTarget: Sendable {
    let destination: String
    let container: String
    let operation: String
    let node: String
    let base: UInt64
    let target: RemoteSave.Target
    let file = UUID()

    func record(_ body: WholeFileStage.Identity) throws -> RemoteSave {
        guard body.id == file else { throw RemoteSave.Failure.invalidIntent }
        return try RemoteSave(destination: destination, container: container, operation: operation,
                              node: node, base: base, target: target, wholeFile: body)
    }
}

final class WholeFileImportController: UIViewController, UIDocumentPickerDelegate {
    private let plan: WholeFileTarget
    private let files: WholeFileStage
    private let drafts: RemoteSaveStore
    private let onPrepared: (RemoteSave, Bool) -> Void
    private let pick = UIButton(type: .system)
    private let confirm = UIButton(type: .system)
    private let notice = UILabel()
    private let queue = OperationQueue()
    private weak var picker: UIDocumentPickerViewController?
    private var current: DocumentRead?
    private var scope: DocumentPresentation?
    private var selection: HeldArchive?
    private var terminal = false

    init(plan: WholeFileTarget, files: WholeFileStage, drafts: RemoteSaveStore,
         onPrepared: @escaping (RemoteSave, Bool) -> Void) {
        self.plan = plan; self.files = files; self.drafts = drafts; self.onPrepared = onPrepared
        super.init(nibName: nil, bundle: nil)
        queue.maxConcurrentOperationCount = 1
        queue.qualityOfService = .userInitiated
    }

    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "准备完整文件"
        view.backgroundColor = .systemBackground
        pick.setTitle("选择文件…", for: .normal)
        pick.addTarget(self, action: #selector(selectFile), for: .touchUpInside)
        confirm.setTitle("确认并暂存完整文件", for: .normal)
        confirm.addTarget(self, action: #selector(prepare), for: .touchUpInside)
        confirm.isEnabled = false
        notice.numberOfLines = 0
        notice.text = "选择文件只做本地准备。提交需另行确认。"
        let stack = UIStackView(arrangedSubviews: [notice, pick, confirm])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        let safe = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: safe.topAnchor, constant: 16),
            stack.leadingAnchor.constraint(equalTo: safe.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: safe.trailingAnchor, constant: -16)
        ])
    }

    private var isActive: Bool {
        navigationController?.topViewController === self &&
            view.window?.windowScene?.activationState == .foregroundActive
    }

    @objc private func selectFile() {
        guard isActive, !terminal, picker == nil, presentedViewController == nil else { return }
        resetSelection()
        let choose = UIDocumentPickerViewController(forOpeningContentTypes: [.data], asCopy: false)
        choose.delegate = self
        choose.allowsMultipleSelection = false
        picker = choose
        present(choose, animated: true)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        if picker === controller { picker = nil }
    }

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard picker === controller else { return }
        picker = nil
        guard isActive, !terminal, urls.count == 1, let url = urls.first else { return }
        let reading = DocumentRead()
        do {
            let selected = try reading.present(url, role: .wholeFile) { [weak self] lost in
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.scope === lost else { return }
                    self.resetSelection()
                    self.notice.text = "原件已变化或不可访问，请重新选择。"
                }
            }
            current = reading
            scope = selected
            pick.isEnabled = false
            queue.addOperation { [weak self] in
                let inspected = Result { try reading.inspectWholeFile(selected) }
                DispatchQueue.main.async { [weak self] in
                    guard let self, self.current === reading, self.scope === selected, self.isActive else { return }
                    self.pick.isEnabled = true
                    if case let .success(held) = inspected {
                        self.selection = held
                        self.confirm.isEnabled = true
                        self.notice.text = "\(url.lastPathComponent) · \(held.size) B\n暂存整份文件，暂不发送。"
                    } else {
                        self.resetSelection()
                        self.notice.text = "无法读取选定的原件。"
                    }
                }
            }
        } catch {
            notice.text = "无法打开选定的原件。"
        }
    }

    @objc private func prepare() {
        guard isActive, !terminal, let reading = current, let selected = selection else { return }
        terminal = true
        confirm.isEnabled = false
        pick.isEnabled = false
        notice.text = "正在检查并暂存整份文件…"
        let plan = plan, files = files, drafts = drafts
        queue.addOperation { [weak self] in
            let result: (RemoteSave, Bool)? = {
                do {
                    let body = try reading.freezeWholeFile(selected, id: plan.file, into: files) { identity in
                        try drafts.retain(plan.record(identity))
                    }
                    return (try plan.record(body), true)
                } catch {
                    guard let record = try? drafts.reopenForRecovery(operation: plan.operation) else { return nil }
                    return (record, false)
                }
            }()
            DispatchQueue.main.async { [weak self] in
                guard let self, self.current === reading, self.isActive else { return }
                self.resetSelection()
                if let (saved, ready) = result {
                    self.onPrepared(saved, ready)
                } else {
                    self.notice.text = "未确认暂存完成，文件未发送。原操作号：\(plan.operation)"
                }
            }
        }
    }

    private func resetSelection() {
        current?.cancel(); current = nil
        scope?.retire(); scope = nil
        selection = nil
        confirm.isEnabled = false
        pick.isEnabled = !terminal
    }

    func leaveForeground() {
        guard isViewLoaded else { return }
        picker = nil
        resetSelection()
        notice.text = terminal ? "已停止处理。返回后可按原操作号查看。\n\(plan.operation)" : "请重新选择文件。"
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent || navigationController?.isBeingDismissed == true { leaveForeground() }
    }
}

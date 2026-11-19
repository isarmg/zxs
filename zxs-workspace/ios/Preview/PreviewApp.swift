import UIKit
import UniformTypeIdentifiers

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {}

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let w = UIWindow(windowScene: scene)
        w.rootViewController = PreviewController()
        self.window = w; w.makeKeyAndVisible()
    }
    func sceneDidEnterBackground(_ scene: UIScene) {
        (window?.rootViewController as? PreviewController)?.clearSelection()
    }
    func sceneDidDisconnect(_ scene: UIScene) {
        (window?.rootViewController as? PreviewController)?.clearSelection()
    }
}

final class PreviewController: UIViewController, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate,
                               UITableViewDataSource, UITableViewDelegate {
    private let queue: OperationQueue = {
        let q = OperationQueue(); q.maxConcurrentOperationCount = 1; q.qualityOfService = .userInitiated; return q
    }()
    private let status = UILabel(), location = UILabel(), table = UITableView()
    private let cancelButton = UIButton(type: .system), up = UIButton(type: .system)
    private let previous = UIButton(type: .system), next = UIButton(type: .system)
    private weak var awaiting: UIDocumentPickerViewController?
    private var active: DocumentRead?
    private var selection: DocumentPresentation?, report: Report?
    private weak var decision: UIAlertController?
    private var trail: [Member] = [], children: [Member] = [], page = 0
    private let pageSize = 100

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let notice = UILabel()
        for label in [notice, status, location] { label.numberOfLines = 0; label.adjustsFontForContentSizeCategory = true }
        notice.font = .preferredFont(forTextStyle: .headline)
        notice.text = "ZIP 目录预览\n目录只显示声明；点普通文件可核验该项 CRC-32，不认证快照。外部提供方可能自行下载。"
        status.font = .preferredFont(forTextStyle: .body)
        location.font = .preferredFont(forTextStyle: .footnote)
        location.numberOfLines = 2
        func wire(_ button: UIButton, _ title: String, _ action: Selector) {
            button.setTitle(title, for: .normal); button.addTarget(self, action: action, for: .touchUpInside)
        }
        let pick = UIButton(type: .system)
        wire(pick, "选择 .zxs 或 Store ZIP", #selector(choose))
        wire(cancelButton, "取消并关闭文件", #selector(cancelRead))
        wire(up, "上一级", #selector(goUp))
        wire(previous, "上一页", #selector(prevPage))
        wire(next, "下一页", #selector(nextPage))
        let nav = UIStackView(arrangedSubviews: [up, previous, next]); nav.distribution = .fillEqually
        let stack = UIStackView(arrangedSubviews: [notice, pick, cancelButton, status, location, nav, table])
        stack.axis = .vertical; stack.spacing = 5; stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([stack.topAnchor.constraint(equalTo: guide.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: guide.bottomAnchor), stack.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -12)])
        table.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        table.dataSource = self; table.delegate = self
        reset("请选择一份文件。")
    }

    func clearSelection() {
        awaiting = nil; active?.cancel(); active = nil
        queue.cancelAllOperations()
        decision?.dismiss(animated: false); decision = nil
        let previous = selection; selection = nil; previous?.retire()
        report = nil
        if isViewLoaded { reset("没有保留上次选择。") }
    }
    @objc private func cancelRead() { clearSelection(); reset("已取消本次读取。") }

    @objc private func choose() {
        guard presentedViewController == nil else { return }
        clearSelection(); reset("等待系统文档选择器…")
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.data], asCopy: false)
        picker.allowsMultipleSelection = false; picker.shouldShowFileExtensions = true
        picker.delegate = self; awaiting = picker
        present(picker, animated: true)
        picker.presentationController?.delegate = self
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard awaiting === controller else { return }
        awaiting = nil; reset("没有选择文件。")
    }
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        if let p = awaiting, presentationController.presentedViewController === p { documentPickerWasCancelled(p) }
    }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard awaiting === controller else { return }
        awaiting = nil
        guard urls.count == 1, let url = urls.first else { reset("仅支持选择一份文件。"); return }
        let job = DocumentRead(), scope: DocumentPresentation
        do {
            scope = try job.present(url) { [weak self] lost in
                DispatchQueue.main.async { [weak self] in
                    guard let self = self, self.selection === lost else { return }
                    self.clearSelection(); self.reset("外部文档已变化或需让出访问。请重新选择。")
                }
            }
        } catch { reset("未获得这份文件的临时读取权。"); return }
        selection = scope; active = job; reset("正在读取目录声明…")
        queue.addOperation { [weak self] in
            let outcome = Result { try job.run(scope) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.selection === scope else { return }
                self.active = nil
                guard (try? scope.check()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let report):
                    self.report = report
                    self.reset(report.name + "（本次目录记录）")
                    self.trail = [report.directory.root]; self.enter()
                case .failure:
                    self.clearSelection()
                    self.reset("无法预览。文件不可用、授权不足、读取时变化或 ZIP 目录超出支持范围。")
                }
            }
        }
    }
    private func offer(_ file: Member) {
        guard let view = report, selection === view.archive.scope, active == nil,
              presentedViewController == nil, (try? view.archive.scope.check()) != nil else { return }
        let path = "/" + (trail.dropFirst().map(\.name) + [file.name]).joined(separator: "/")
        let alert = UIAlertController(title: "核验这一项的 CRC-32？",
            message: "\(view.name)\n\(path)\n将读取全部 \(file.size) 字节；不保存或上传。外部提供方可能自行下载。",
            preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "返回目录", style: .cancel) { [weak self] _ in self?.decision = nil })
        alert.addAction(UIAlertAction(title: "开始核验", style: .default) { [weak self] _ in
            guard let self = self else { return }; self.decision = nil
            guard self.selection === view.archive.scope, self.active == nil,
                  (try? view.archive.scope.check()) != nil else { return }
            self.verify(view, file: file, path: path)
        })
        decision = alert; present(alert, animated: true)
    }
    private func verify(_ view: Report, file: Member, path: String) {
        let job = DocumentRead(); active = job
        status.text = "正在读取 \(path)（\(file.size) B）…"; render()
        queue.addOperation { [weak self] in
            let outcome = Result { try job.verify(view, file: file) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.selection === view.archive.scope else { return }
                self.active = nil
                guard (try? view.archive.scope.check()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let result):
                    let verdict = result.matches ? "该项 CRC-32 与 ZIP 声明相符。" : "该项 CRC-32 与 ZIP 声明不符。"
                    let values = String(format: "声明 %08X，实读 %08X。", result.declared, result.actual)
                    self.status.text = "\(view.name)\n\(path)（\(file.size) B）\n\(verdict) \(values) 不证明整包或其他项。"
                    self.render()
                case .failure:
                    self.clearSelection()
                    self.reset("没有完整核验结果：布局不符、原文件变化或读取不可用。请重新选择。")
                }
            }
        }
    }
    private func reset(_ message: String) {
        trail = []; children = []; page = 0
        status.text = message; location.text = nil; cancelButton.isHidden = active == nil
        up.isEnabled = false; previous.isEnabled = false; next.isEnabled = false
        table.reloadData()
    }
    private func enter() {
        page = 0
        if let rows = trail.last?.children {
            children = rows.values.sorted { $0.name.utf8.lexicographicallyPrecedes($1.name.utf8) }
        } else { children = [] }
        render()
    }
    private func render() {
        let pages = max(1, (children.count + pageSize - 1) / pageSize)
        location.text = "/" + trail.dropFirst().map(\.name).joined(separator: "/") + "   " +
            (children.isEmpty ? "空目录" : "\(page + 1)/\(pages) 页")
        let idle = active == nil
        table.allowsSelection = idle; cancelButton.isHidden = idle
        up.isEnabled = idle && trail.count > 1; previous.isEnabled = idle && page > 0
        next.isEnabled = idle && page + 1 < pages
        table.reloadData()
        if table.numberOfRows(inSection: 0) != 0 {
            table.scrollToRow(at: IndexPath(row: 0, section: 0), at: .top, animated: false)
        }
    }
    @objc private func goUp() { if trail.count > 1 { trail.removeLast(); enter() } }
    @objc private func prevPage() { if page > 0 { page -= 1; render() } }
    @objc private func nextPage() { if (page + 1) * pageSize < children.count { page += 1; render() } }
    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        min(pageSize, max(0, children.count - page * pageSize))
    }
    func tableView(_ tableView: UITableView, cellForRowAt path: IndexPath) -> UITableViewCell {
        let row = children[page * pageSize + path.row], cell = tableView.dequeueReusableCell(withIdentifier: "row", for: path)
        var content = cell.defaultContentConfiguration()
        content.text = row.name
        content.secondaryText = row.children == nil ? "\(row.size) B（声明）" : "目录"
        cell.contentConfiguration = content
        cell.accessoryType = row.children == nil ? .none : .disclosureIndicator
        cell.selectionStyle = .default
        return cell
    }
    func tableView(_ tableView: UITableView, didSelectRowAt path: IndexPath) {
        let i = page * pageSize + path.row
        guard active == nil, children.indices.contains(i) else { return }
        let row = children[i]
        if row.children != nil { trail.append(row); enter() } else { offer(row) }
    }
    deinit { active?.cancel(); queue.cancelAllOperations(); selection?.retire() }
}

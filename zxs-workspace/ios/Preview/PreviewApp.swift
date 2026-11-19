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
    private var trail: [Member] = [], children: [Member] = [], page = 0
    private let pageSize = 100

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let notice = UILabel()
        for label in [notice, status, location] { label.numberOfLines = 0; label.adjustsFontForContentSizeCategory = true }
        notice.font = .preferredFont(forTextStyle: .headline)
        notice.text = "ZIP 目录预览\n只查看这次读取的名称和声明大小，不核验内容或快照。选择外部文件可能触发系统下载。"
        status.font = .preferredFont(forTextStyle: .body)
        location.font = .preferredFont(forTextStyle: .footnote)
        location.numberOfLines = 2
        func wire(_ button: UIButton, _ title: String, _ action: Selector) {
            button.setTitle(title, for: .normal); button.addTarget(self, action: action, for: .touchUpInside)
        }
        let pick = UIButton(type: .system)
        wire(pick, "选择 .zxs 或 Store ZIP", #selector(choose))
        wire(cancelButton, "取消读取", #selector(cancelRead))
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
        let job = DocumentRead()
        active = job; reset("正在读取目录声明…")
        queue.addOperation { [weak self] in
            let outcome = Result { try job.run(url) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job else { return }
                self.active = nil
                switch outcome {
                case .success(let report):
                    self.reset(report.name + "（本次目录记录）")
                    self.trail = [report.directory.root]; self.enter()
                case .failure:
                    self.reset("无法预览。文件不可用、授权不足、读取时变化或 ZIP 目录超出支持范围。")
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
        up.isEnabled = trail.count > 1; previous.isEnabled = page > 0; next.isEnabled = page + 1 < pages
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
        cell.selectionStyle = row.children == nil ? .none : .default
        return cell
    }
    func tableView(_ tableView: UITableView, didSelectRowAt path: IndexPath) {
        let i = page * pageSize + path.row
        if children.indices.contains(i), children[i].children != nil { trail.append(children[i]); enter() }
    }
    deinit { active?.cancel(); queue.cancelAllOperations() }
}

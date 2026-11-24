import CryptoKit
import Foundation
import RemoteTransport
import UIKit

struct MobileSaveCredential: Sendable {
    let username: String
    let password: String

    static func validLogin(_ value: String) -> Bool {
        let utf8 = value.utf8
        func asciiLowerOrDigit(_ byte: UInt8) -> Bool { (97...122).contains(byte) || (48...57).contains(byte) }
        return (1...64).contains(utf8.count) && utf8.first.map(asciiLowerOrDigit) == true
            && utf8.allSatisfy { asciiLowerOrDigit($0) || [46, 95, 45].contains($0) }
    }

    init(username: String, password: String) throws {
        guard Self.validLogin(username), (1...1024).contains(password.utf8.count),
              !password.utf8.contains(where: { $0 <= 31 || $0 == 127 })
        else { throw RemoteSave.Failure.invalidIntent }
        self.username = username
        self.password = password
    }

    func basic() -> String { "Basic " + Data((username + ":" + password).utf8).base64EncodedString() }
}

struct MobileSaveExchange: RemoteSaveExchange {
    let endpoint: R3HTTPSOrigin
    let credential: MobileSaveCredential
    let client: R3HTTPSClient

    var destination: String { Self.identity(endpoint: endpoint, user: credential.username) }

    static func identity(endpoint: R3HTTPSOrigin, user: String) -> String {
        var material = Data(("zxs-mobile-r3\0" + endpoint.identity + "\0").utf8)
        material.append(contentsOf: user.utf8)
        return SHA256.hash(data: material).map { String(format: "%02x", $0) }.joined()
    }

    func once(_ request: RemoteSave.Request) async throws -> RemoteSavePacket {
        guard request.frozenFile == nil else { throw RemoteSave.Failure.invalidIntent }
        let message = R3Request(method: request.method, path: request.path, headers: request.headers, body: request.body)
        let response = try await read(message)
        return RemoteSavePacket(status: response.status, bytes: response.bytes)
    }

    func read(_ request: R3Request) async throws -> R3Response {
        try await client.once(request, to: endpoint, authorization: { credential.basic() })
    }
}

@MainActor
private final class MobileSaveSession {
    static let shared: Result<MobileSaveSession, Error> = Result {
        try MobileSaveSession(store: .applicationStore())
    }

    let store: RemoteSaveStore
    let coordinator: RemoteSaveCoordinator

    private init(store: RemoteSaveStore) {
        self.store = store
        coordinator = RemoteSaveCoordinator(store: store)
    }
}

final class RemoteDraftController: UIViewController {
    private let draft: LocalDraft.Revision?
    private let origin = UITextField(), user = UITextField(), password = UITextField()
    private let fileName = UITextField(), operation = UITextField()
    private let status = UILabel(), pick = UIButton(type: .system)
    private var controls: [UIButton] = []
    private var job: Task<Void, Never>?
    private weak var confirmation: UIAlertController?

    init(draft: LocalDraft.Revision?) {
        self.draft = draft
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "远程提交"
        view.backgroundColor = .systemBackground
        configure(origin, "HTTPS 服务器，例如 https://store.example:443")
        origin.keyboardType = .URL
        configure(user, "服务器账号")
        configure(password, "服务器密码")
        password.isSecureTextEntry = true
        configure(fileName, "新文件名")
        configure(operation, "原操作号")
        fileName.isEnabled = draft != nil

        let prepare = button("准备新文件", #selector(prepareNew))
        prepare.isEnabled = draft != nil
        pick.setTitle("选取已准备记录", for: .normal)
        pick.showsMenuAsPrimaryAction = true
        let lookup = button("查原回执", #selector(lookupOnly))
        let submit = button("首次提交", #selector(submitFirst))
        let retry = button("按原号重试", #selector(retryOriginal))
        let browse = button("浏览服务器文件…", #selector(openBrowse))
        controls = [prepare, pick, lookup, submit, retry, browse]
        status.numberOfLines = 0
        status.font = .preferredFont(forTextStyle: .footnote)
        status.adjustsFontForContentSizeCategory = true
        status.text = draft == nil ? "可按原操作号查询或重试。" : "准备本地记录后，才可明确提交。"

        let scroll = UIScrollView()
        let content = UIStackView(arrangedSubviews: [origin, user, password, fileName, prepare, operation,
                                                    pick, lookup, submit, retry, browse, status])
        content.axis = .vertical
        content.spacing = 10
        scroll.keyboardDismissMode = .interactive
        scroll.translatesAutoresizingMaskIntoConstraints = false
        content.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)
        scroll.addSubview(content)
        let safe = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: safe.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: safe.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: safe.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 12),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -16),
            content.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 16),
            content.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -16),
            content.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -32)
        ])
        refreshPicker()
    }

    private func configure(_ field: UITextField, _ label: String) {
        field.placeholder = label
        field.accessibilityLabel = label
        field.borderStyle = .roundedRect
        field.autocapitalizationType = .none
        field.autocorrectionType = .no
        field.spellCheckingType = .no
        field.smartQuotesType = .no
        field.smartDashesType = .no
        field.clearButtonMode = .whileEditing
        field.font = .preferredFont(forTextStyle: .body)
        field.adjustsFontForContentSizeCategory = true
    }

    private func button(_ title: String, _ action: Selector) -> UIButton {
        let view = UIButton(type: .system)
        view.setTitle(title, for: .normal)
        view.titleLabel?.font = .preferredFont(forTextStyle: .body)
        view.titleLabel?.adjustsFontForContentSizeCategory = true
        view.addTarget(self, action: action, for: .touchUpInside)
        return view
    }

    private func refreshPicker() {
        let available = (try? MobileSaveSession.shared.get().store.operationIDs()) ?? []
        pick.isEnabled = !available.isEmpty && job == nil
        pick.menu = UIMenu(children: available.prefix(64).map { id in
            UIAction(title: id) { [weak self] _ in
                self?.operation.text = id
                if let save = try? MobileSaveSession.shared.get().store.reopenForRecovery(operation: id) {
                    self?.status.text = save.summary
                }
            }
        })
        if available.count > 64 { pick.setTitle("选取记录（其余可输入原操作号）", for: .normal) }
    }

    private func selectedOrigin() throws -> (R3HTTPSOrigin, String) {
        let endpoint = try R3HTTPSOrigin(origin.text ?? "")
        let account = user.text ?? ""
        guard MobileSaveCredential.validLogin(account) else { throw RemoteSave.Failure.invalidIntent }
        return (endpoint, account)
    }

    @objc private func prepareNew() {
        guard job == nil, let draft, view.window?.windowScene?.activationState == .foregroundActive else { return }
        do {
            let (endpoint, account) = try selectedOrigin()
            let save = try RemoteSave(destination: MobileSaveExchange.identity(endpoint: endpoint, user: account),
                container: Self.newID(), operation: Self.newID(), node: Self.newID(), base: 0,
                target: .firstMain(name: fileName.text ?? ""), draft: draft)
            try MobileSaveSession.shared.get().store.retain(save)
            operation.text = save.operation
            status.text = "已准备本地记录\n" + save.summary
            refreshPicker()
        } catch {
            status.text = "未能准备记录。请检查服务器、账号、文件名和本机存储。"
        }
    }

    @objc private func openBrowse() {
        guard job == nil, view.window?.windowScene?.activationState == .foregroundActive else { return }
        do {
            let (endpoint, account) = try selectedOrigin()
            let credential = try MobileSaveCredential(username: account, password: password.text ?? "")
            let session = try MobileSaveSession.shared.get()
            password.text = nil
            let exchange = MobileSaveExchange(endpoint: endpoint, credential: credential, client: R3HTTPSClient())
            let browser = RemoteBrowseController(exchange: exchange, store: session.store) { [weak self] saved in
                guard let self else { return }
                self.operation.text = saved.operation
                self.status.text = "已准备完整文件。请输入原服务器密码，再明确提交。\n" + saved.summary
                self.refreshPicker()
                self.navigationController?.popToViewController(self, animated: true)
            }
            navigationController?.pushViewController(browser, animated: true)
        } catch {
            status.text = "请检查服务器、账号与密码。"
        }
    }

    static func newID() -> String { UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "") }

    @objc private func lookupOnly() { offer(.lookup) }
    @objc private func submitFirst() { offer(.submitFirst) }
    @objc private func retryOriginal() { offer(.retrySameOperation) }

    private func offer(_ choice: RemoteSaveCoordinator.Choice) {
        guard job == nil, presentedViewController == nil,
              view.window?.windowScene?.activationState == .foregroundActive else { return }
        do {
            let (endpoint, account) = try selectedOrigin()
            let credential = try MobileSaveCredential(username: account, password: password.text ?? "")
            let session = try MobileSaveSession.shared.get()
            let id = operation.text ?? ""
            let save = try session.store.reopenForRecovery(operation: id)
            let expected = MobileSaveExchange.identity(endpoint: endpoint, user: account)
            _ = try save.lookup(for: expected)
            let consent = try save.confirmation()
            let actionTitle: String
            switch choice {
            case .lookup: actionTitle = "查原回执"
            case .submitFirst: actionTitle = "上传这份完整文件"
            case .retrySameOperation: actionTitle = "按原号重试完整文件"
            }
            let confirmer = UIAlertController(title: actionTitle,
                message: endpoint.identity + "\n" + account + "\n" + save.summary, preferredStyle: .alert)
            confirmer.addAction(UIAlertAction(title: "取消", style: .cancel) { [weak self] _ in self?.confirmation = nil })
            confirmer.addAction(UIAlertAction(title: actionTitle, style: .default) { [weak self, weak confirmer] _ in
                guard let self, let confirmer, self.confirmation === confirmer,
                      self.view.window?.windowScene?.activationState == .foregroundActive else { return }
                self.confirmation = nil
                let exchange = MobileSaveExchange(endpoint: endpoint, credential: credential, client: R3HTTPSClient())
                self.run(choice, consent: consent, session: session, exchange: exchange)
            })
            confirmation = confirmer
            present(confirmer, animated: true)
        } catch {
            status.text = "请检查账号、密码和原操作号。查询和提交必须使用准备记录时的服务器与账号。"
        }
    }

    private func run(_ choice: RemoteSaveCoordinator.Choice, consent: RemoteSaveConsent, session: MobileSaveSession,
                     exchange: MobileSaveExchange) {
        guard job == nil else { return }
        let id = consent.operation
        password.text = nil
        controls.forEach { $0.isEnabled = false }
        status.text = "正在核对原操作…"
        job = Task { [weak self] in
            defer {
                self?.job = nil
                self?.controls.forEach { $0.isEnabled = true }
                self?.controls.first?.isEnabled = self?.draft != nil
                self?.refreshPicker()
            }
            do {
                let result = try await session.coordinator.run(consent: consent, choice: choice, over: exchange)
                guard !Task.isCancelled else { return }
                switch result {
                case .committed(let sequence): self?.status.text = "已确认保存，版本 \(sequence)\n\(id)"
                case .absent: self?.status.text = "服务器未找到原操作；本次未上传。\n\(id)"
                case .requiresExplicitRetry: self?.status.text = "原请求曾尝试发送。需要时请选择「按原号重试」。\n\(id)"
                case .conflict: self?.status.text = "服务器报告冲突，已保留原操作。\n\(id)"
                case .unknown: self?.status.text = "没有取得匹配的回执。可按原号查询。\n\(id)"
                }
            } catch {
                if !Task.isCancelled { self?.status.text = "未取得完整回执，可按原号查询。不会自动重发。\n\(id)" }
            }
        }
    }

    func leaveForeground() {
        confirmation?.dismiss(animated: false)
        confirmation = nil
        job?.cancel()
        password.text = nil
        if job != nil { status.text = "已停止等待结果，可按原号查询。" }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent || navigationController?.isBeingDismissed == true { leaveForeground() }
    }
}

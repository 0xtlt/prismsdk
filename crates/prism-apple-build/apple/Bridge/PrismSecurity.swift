// Secrets, the device owner and sign-in for the Rust app (prism_core::security):
// the Keychain (Security), LocalAuthentication, Sign in with Apple and
// ASWebAuthenticationSession (AuthenticationServices).
//
// Availability, from the SDK headers:
// - LocalAuthentication: no framework on tvOS. watchOS has
//   LAPolicyDeviceOwnerAuthentication (passcode, wrist detection) but no
//   biometric policy; LABiometryType exists from watchOS 11 / tvOS 18.
// - kSecAccessControlBiometryCurrentSet: iOS, macOS (and visionOS); watchOS
//   uses kSecAccessControlUserPresence.
// - ASWebAuthenticationSession: iOS 12, macOS 10.15, watchOS 6.2, tvOS 16;
//   presentationContextProvider is iOS/macOS only.
// - ASAuthorizationAppleIDProvider: iOS 13, macOS 10.15, tvOS 13, watchOS 6.
import AuthenticationServices
import Foundation
#if canImport(LocalAuthentication)
import LocalAuthentication
#endif
// tvOS SDKs ship LocalAuthentication headers with LAContext marked unavailable:
// `os(tvOS)` is excluded everywhere below.
import Security
#if canImport(UIKit)
import UIKit
#elseif canImport(AppKit)
import AppKit
#endif

// MARK: Answers (implemented in Rust, prism-native apple.rs)

@_silgen_name("prism_device_reply_text")
private func replyText(_ id: UInt64, _ text: UnsafePointer<CChar>?)
@_silgen_name("prism_device_reply_error")
private func replyError(_ id: UInt64, _ event: Bool, _ message: UnsafePointer<CChar>)
/// A JSON object of strings.
@_silgen_name("prism_device_reply_fields")
private func replyFields(_ id: UInt64, _ json: UnsafePointer<CChar>)

private func text(_ id: UInt64, _ value: String?) {
    DispatchQueue.main.async {
        if let value { value.withCString { replyText(id, $0) } } else { replyText(id, nil) }
    }
}

private func failure(_ id: UInt64, _ message: String) {
    DispatchQueue.main.async { message.withCString { replyError(id, false, $0) } }
}

private func fields(_ id: UInt64, _ values: [String: String]) {
    let data = (try? JSONSerialization.data(withJSONObject: values)) ?? Data("{}".utf8)
    let json = String(decoding: data, as: UTF8.self)
    DispatchQueue.main.async { json.withCString { replyFields(id, $0) } }
}

private func done(_ id: UInt64) { text(id, nil) }

private func str(_ p: UnsafePointer<CChar>?) -> String? { p.map { String(cString: $0) } }

private func info(_ key: String) -> String? { Bundle.main.object(forInfoDictionaryKey: key) as? String }

// MARK: Keychain

/// Keychain items of this app: its bundle id (or executable name when it
/// runs unbundled, like `cargo run` on macOS).
private var service: String {
    Bundle.main.bundleIdentifier ?? "prism.\(ProcessInfo.processInfo.processName)"
}

private let keychainQueue = DispatchQueue(label: "prism.keychain")

private func baseQuery(_ key: String, shared: Bool, dataProtection: Bool) -> [String: Any] {
    var q: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: service,
        kSecAttrAccount as String: key,
    ]
    // The App Group doubles as a keychain access group (shared with widgets).
    if shared, let group = info("PrismAppGroup") { q[kSecAttrAccessGroup as String] = group }
    #if os(macOS)
    if dataProtection { q[kSecUseDataProtectionKeychain as String] = true }
    #endif
    return q
}

/// Marks items that need the owner (kSecAttrGeneric): reading one first shows
/// the prompt (LAContext.evaluateAccessControl), then hands the keychain the
/// authenticated context. macOS apps without a provisioning profile cannot use
/// the data protection keychain (errSecMissingEntitlement): their biometric
/// items live in the login keychain, and the app checks the owner itself.
private let gateMarker = "prism.owner-gate"

private func status(_ s: OSStatus) -> String {
    switch s {
    case errSecUserCanceled: return "cancelled"
    case errSecAuthFailed: return "denied"
    case errSecMissingEntitlement:
        return "keychain: missing entitlement (sign the app with a provisioning profile)"
    default:
        let message = SecCopyErrorMessageString(s, nil) as String? ?? "error"
        return "keychain: \(message) (\(s))"
    }
}

#if canImport(LocalAuthentication) && !os(tvOS)
/// Biometrics are enrolled and usable now.
private func biometricsEnrolled() -> Bool {
    #if os(watchOS)
    return false
    #else
    return LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    #endif
}
#endif

#if os(tvOS)
private func accessControl() -> SecAccessControl? { nil }
#else
/// Reading needs the owner: the current biometric set if enrolled (re-enrolling
/// invalidates it), the passcode otherwise.
private func accessControl() -> SecAccessControl? {
    #if os(watchOS)
    let flags: SecAccessControlCreateFlags = .userPresence
    #else
    let flags: SecAccessControlCreateFlags = biometricsEnrolled() ? .biometryCurrentSet : .userPresence
    #endif
    return SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, flags, nil)
}
#endif

@_cdecl("prism_secure_set")
public func prismSecureSet(_ id: UInt64, _ key: UnsafePointer<CChar>, _ value: UnsafePointer<CChar>, _ biometric: Bool, _ shared: Bool) {
    let key = String(cString: key)
    let data = Data(String(cString: value).utf8)
    keychainQueue.async {
        // Access control cannot be updated in place: replace the item.
        for dp in [false, true] { SecItemDelete(baseQuery(key, shared: shared, dataProtection: dp) as CFDictionary) }
        var add = baseQuery(key, shared: shared, dataProtection: biometric)
        add[kSecValueData as String] = data
        if biometric {
            #if os(tvOS)
            return failure(id, "unavailable") // no LocalAuthentication on tvOS
            #else
            guard let access = accessControl() else { return failure(id, "unavailable") }
            add[kSecAttrAccessControl as String] = access
            add[kSecAttrGeneric as String] = Data(gateMarker.utf8)
            #endif
        } else {
            // This device only: not in backups, not migrated.
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        }
        var s = SecItemAdd(add as CFDictionary, nil)
        #if os(macOS)
        if biometric && s == errSecMissingEntitlement {
            var legacy = baseQuery(key, shared: shared, dataProtection: false)
            legacy[kSecValueData as String] = data
            legacy[kSecAttrGeneric as String] = Data(gateMarker.utf8)
            s = SecItemAdd(legacy as CFDictionary, nil)
        }
        #endif
        if s == errSecSuccess { done(id) } else { failure(id, status(s)) }
    }
}

@_cdecl("prism_secure_get")
public func prismSecureGet(_ id: UInt64, _ key: UnsafePointer<CChar>, _ prompt: UnsafePointer<CChar>?, _ shared: Bool) {
    let key = String(cString: key)
    let prompt = str(prompt)
    keychainQueue.async {
        // Data protection keychain first (biometric items on macOS), then the default.
        for dp in [true, false] {
            #if !os(macOS)
            if dp { continue }
            #endif
            // 1. Attributes only: never prompts.
            var q = baseQuery(key, shared: shared, dataProtection: dp)
            q[kSecReturnAttributes as String] = true
            q[kSecMatchLimit as String] = kSecMatchLimitOne
            var item: CFTypeRef?
            let s = SecItemCopyMatching(q as CFDictionary, &item)
            if s == errSecItemNotFound || s == errSecMissingEntitlement { continue }
            guard s == errSecSuccess, let attrs = item as? [String: Any] else { return failure(id, status(s)) }
            let gated = (attrs[kSecAttrGeneric as String] as? Data) == Data(gateMarker.utf8)
            // 2. The value, with the context that proved the owner.
            #if canImport(LocalAuthentication) && !os(tvOS)
            let context = LAContext()
            let read = { readValue(id, key, shared: shared, dataProtection: dp, context: context) }
            guard gated else { return read() }
            let reason = (prompt?.isEmpty == false ? prompt : nil) ?? "Unlock a secret"
            // Simulators neither enforce keychain access control nor support
            // evaluateAccessControl (LocalAuthentication error -1020): check
            // the owner with the policy there.
            #if targetEnvironment(simulator)
            let simulator = true
            #else
            let simulator = false
            #endif
            if !simulator, let raw = attrs[kSecAttrAccessControl as String], CFGetTypeID(raw as CFTypeRef) == SecAccessControlGetTypeID() {
                // Shows Face ID / Touch ID now; the keychain then accepts this context.
                let access = raw as! SecAccessControl
                context.evaluateAccessControl(access, operation: .useItem, localizedReason: reason) { ok, error in
                    if ok { keychainQueue.async(execute: read) } else { failure(id, error.map(laError) ?? "denied") }
                }
            } else {
                // The owner check the keychain does not enforce here (simulator, unsigned macOS app).
                ownerCheck(reason: reason, passcode: true) { error in
                    if let error { failure(id, error) } else { keychainQueue.async(execute: read) }
                }
            }
            return
            #else
            if gated { return failure(id, "unavailable") }
            return readValue(id, key, shared: shared, dataProtection: dp, context: nil)
            #endif
        }
        text(id, nil)
    }
}

private func readValue(_ id: UInt64, _ key: String, shared: Bool, dataProtection: Bool, context: AnyObject?) {
    var q = baseQuery(key, shared: shared, dataProtection: dataProtection)
    q[kSecReturnData as String] = true
    q[kSecMatchLimit as String] = kSecMatchLimitOne
    if let context { q[kSecUseAuthenticationContext as String] = context }
    var item: CFTypeRef?
    let s = SecItemCopyMatching(q as CFDictionary, &item)
    if s == errSecItemNotFound { return text(id, nil) }
    guard s == errSecSuccess, let data = item as? Data else { return failure(id, status(s)) }
    text(id, String(decoding: data, as: UTF8.self))
}

@_cdecl("prism_secure_delete")
public func prismSecureDelete(_ id: UInt64, _ key: UnsafePointer<CChar>, _ shared: Bool) {
    let key = String(cString: key)
    keychainQueue.async {
        var result: OSStatus = errSecSuccess
        for dp in [false, true] {
            let s = SecItemDelete(baseQuery(key, shared: shared, dataProtection: dp) as CFDictionary)
            if s != errSecSuccess && s != errSecItemNotFound && s != errSecMissingEntitlement { result = s }
        }
        if result == errSecSuccess { done(id) } else { failure(id, status(result)) }
    }
}

// MARK: LocalAuthentication

#if canImport(LocalAuthentication) && !os(tvOS)
private func kindName(_ context: LAContext) -> String {
    #if os(watchOS)
    return "none" // no biometric sensor on the watch (the biometric policy is unavailable)
    #else
    switch context.biometryType {
    case .touchID: return "touch_id"
    case .faceID: return "face_id"
    case .opticID: return "optic_id"
    default: return "none"
    }
    #endif
}

private func laError(_ error: Error) -> String {
    guard let e = error as? LAError else { return error.localizedDescription }
    switch e.code {
    case .userCancel, .appCancel, .systemCancel, .userFallback: return "cancelled"
    case .authenticationFailed: return "denied"
    case .passcodeNotSet: return "unavailable"
    #if !os(watchOS)
    case .biometryNotAvailable, .biometryNotEnrolled: return "unavailable"
    case .biometryLockout: return "biometrics locked: use the passcode"
    #endif
    default: return e.localizedDescription
    }
}

/// Runs `reply` with nil when the owner authenticated, else the error.
private func ownerCheck(reason: String, passcode: Bool, reply: @escaping (String?) -> Void) {
    let context = LAContext()
    #if os(watchOS)
    let policy: LAPolicy = .deviceOwnerAuthentication // passcode or wrist detection
    #else
    let policy: LAPolicy = passcode ? .deviceOwnerAuthentication : .deviceOwnerAuthenticationWithBiometrics
    #endif
    var error: NSError?
    guard context.canEvaluatePolicy(policy, error: &error) else {
        return reply(error.map(laError) ?? "unavailable")
    }
    #if os(iOS) || os(visionOS)
    // iOS stops apps that use Face ID without explaining why.
    if context.biometryType == .faceID && Bundle.main.object(forInfoDictionaryKey: "NSFaceIDUsageDescription") == nil {
        return reply("add `face_id = \"why the app uses Face ID\"` under [permissions] in Prism.toml (Info.plist NSFaceIDUsageDescription)")
    }
    #endif
    context.evaluatePolicy(policy, localizedReason: reason.isEmpty ? " " : reason) { ok, error in
        reply(ok ? nil : error.map(laError) ?? "denied")
    }
}
#endif

/// 0 none, 1 Touch ID, 2 Face ID, 3 Optic ID (the hardware, enrolled or not).
@_cdecl("prism_biometry_kind")
public func prismBiometryKind() -> Int32 {
    #if canImport(LocalAuthentication) && !os(tvOS)
    let context = LAContext()
    #if !os(watchOS)
    _ = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil)
    #endif
    switch kindName(context) {
    case "touch_id": return 1
    case "face_id": return 2
    case "optic_id": return 3
    default: return 0
    }
    #else
    return 0
    #endif
}

/// The owner can authenticate (biometrics or a passcode).
@_cdecl("prism_device_auth_available")
public func prismDeviceAuthAvailable() -> Bool {
    #if canImport(LocalAuthentication) && !os(tvOS)
    return LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: nil)
    #else
    return false
    #endif
}

@_cdecl("prism_biometry_status")
public func prismBiometryStatus(_ id: UInt64) {
    #if canImport(LocalAuthentication) && !os(tvOS)
    let context = LAContext()
    var enrolled = false, locked = false
    #if !os(watchOS)
    var error: NSError?
    enrolled = context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error)
    locked = (error as? LAError)?.code == .biometryLockout
    #endif
    var passcodeError: NSError?
    let passcode = LAContext().canEvaluatePolicy(.deviceOwnerAuthentication, error: &passcodeError)
        || ((passcodeError as? LAError).map { $0.code != .passcodeNotSet } ?? false)
    fields(id, ["kind": kindName(context), "enrolled": enrolled ? "1" : "0",
                "passcode": passcode ? "1" : "0", "locked_out": locked ? "1" : "0"])
    #else
    fields(id, ["kind": "none", "enrolled": "0", "passcode": "0", "locked_out": "0"])
    #endif
}

@_cdecl("prism_authenticate")
public func prismAuthenticate(_ id: UInt64, _ reason: UnsafePointer<CChar>, _ passcode: Bool) {
    #if canImport(LocalAuthentication) && !os(tvOS)
    ownerCheck(reason: String(cString: reason), passcode: passcode) { error in
        if let error { failure(id, error) } else { done(id) }
    }
    #else
    failure(id, "unavailable") // tvOS: no LocalAuthentication
    #endif
}

// MARK: Presentation

#if os(iOS) || os(visionOS) || os(tvOS)
private func anchor() -> ASPresentationAnchor {
    let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
    return windows.first(where: \.isKeyWindow) ?? windows.first ?? ASPresentationAnchor()
}
#elseif os(macOS)
private func anchor() -> ASPresentationAnchor {
    NSApp.keyWindow ?? NSApp.windows.first ?? ASPresentationAnchor()
}
#endif

// MARK: Sign in with Apple

private var appleRequests: [UInt64: PrismAppleSignIn] = [:]

final class PrismAppleSignIn: NSObject, ASAuthorizationControllerDelegate {
    let id: UInt64
    var controller: ASAuthorizationController?
    init(id: UInt64) { self.id = id }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization) {
        defer { appleRequests[id] = nil }
        guard let c = authorization.credential as? ASAuthorizationAppleIDCredential else {
            return failure(id, "unexpected credential")
        }
        var v: [String: String] = ["user": c.user]
        if let t = c.identityToken { v["identity_token"] = String(decoding: t, as: UTF8.self) }
        if let code = c.authorizationCode { v["authorization_code"] = String(decoding: code, as: UTF8.self) }
        if let email = c.email { v["email"] = email }
        if let n = c.fullName?.givenName { v["given_name"] = n }
        if let n = c.fullName?.familyName { v["family_name"] = n }
        if let s = c.state { v["state"] = s }
        fields(id, v)
    }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
        defer { appleRequests[id] = nil }
        let code = (error as? ASAuthorizationError)?.code
        if code == .canceled { return failure(id, "cancelled") }
        if code == .unknown {
            // What AuthenticationServices reports without the entitlement.
            return failure(id, "Sign in with Apple failed: the app needs the com.apple.developer.applesignin entitlement ([ios] sign_in_with_apple = true and a provisioning profile with the capability) (\(error.localizedDescription))")
        }
        failure(id, error.localizedDescription)
    }
}

#if !os(watchOS)
extension PrismAppleSignIn: ASAuthorizationControllerPresentationContextProviding {
    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor { anchor() }
}
#endif

@_cdecl("prism_apple_sign_in")
public func prismAppleSignIn(_ id: UInt64, _ email: Bool, _ name: Bool, _ nonce: UnsafePointer<CChar>?, _ state: UnsafePointer<CChar>?) {
    let request = ASAuthorizationAppleIDProvider().createRequest()
    var scopes: [ASAuthorization.Scope] = []
    if email { scopes.append(.email) }
    if name { scopes.append(.fullName) }
    request.requestedScopes = scopes
    request.nonce = str(nonce)
    request.state = str(state)
    let delegate = PrismAppleSignIn(id: id)
    let controller = ASAuthorizationController(authorizationRequests: [request])
    controller.delegate = delegate
    #if !os(watchOS)
    controller.presentationContextProvider = delegate
    #endif
    delegate.controller = controller
    appleRequests[id] = delegate
    controller.performRequests()
}

/// The bundle declares the entitlement (Info.plist `PrismAppleSignIn`, set by
/// `prism` with `[ios] sign_in_with_apple = true`).
@_cdecl("prism_apple_sign_in_available")
public func prismAppleSignInAvailable() -> Bool {
    Bundle.main.object(forInfoDictionaryKey: "PrismAppleSignIn") != nil
}

// MARK: Web sign-in (OAuth / OpenID Connect)

private var webSessions: [UInt64: PrismWebAuth] = [:]

final class PrismWebAuth: NSObject {
    var session: ASWebAuthenticationSession?
}

#if os(iOS) || os(visionOS) || os(macOS)
extension PrismWebAuth: ASWebAuthenticationPresentationContextProviding {
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor { anchor() }
}
#endif

/// The app's own scheme (`prism-<app>`), or `preferred` when it is a custom
/// scheme: ASWebAuthenticationSession catches any scheme but http(s).
@_cdecl("prism_auth_redirect")
public func prismAuthRedirect(_ id: UInt64, _ preferred: UnsafePointer<CChar>?) {
    if let p = str(preferred), let scheme = URL(string: p)?.scheme?.lowercased(), scheme != "http", scheme != "https" {
        return text(id, p)
    }
    // Unbundled (`cargo run`): the bundlers' scheme, from the executable
    // (the package name, `_` written `-`: schemes have no underscore).
    let scheme = info("PrismURLScheme")
        ?? "prism-\(ProcessInfo.processInfo.processName.lowercased().replacingOccurrences(of: "_", with: "-"))"
    text(id, "\(scheme):/oauth")
}

@_cdecl("prism_web_auth")
public func prismWebAuth(_ id: UInt64, _ url: UnsafePointer<CChar>, _ redirect: UnsafePointer<CChar>, _ ephemeral: Bool) {
    guard let url = URL(string: String(cString: url)), let scheme = URL(string: String(cString: redirect))?.scheme else {
        return failure(id, "invalid sign-in URL")
    }
    let holder = PrismWebAuth()
    let session = ASWebAuthenticationSession(url: url, callbackURLScheme: scheme) { callback, error in
        DispatchQueue.main.async { webSessions[id] = nil }
        if let callback { return text(id, callback.absoluteString) }
        if let e = error as? ASWebAuthenticationSessionError, e.code == .canceledLogin { return failure(id, "cancelled") }
        failure(id, error?.localizedDescription ?? "cancelled")
    }
    #if os(iOS) || os(visionOS) || os(macOS)
    session.presentationContextProvider = holder
    #endif
    #if !os(tvOS)
    session.prefersEphemeralWebBrowserSession = ephemeral
    #endif
    holder.session = session
    webSessions[id] = holder
    if !session.start() {
        webSessions[id] = nil
        failure(id, "could not start the sign-in session")
    }
}

// MARK: Passkeys of the app's own relying party (#94)
//
// The server's WebAuthn options (JSON, binary fields in base64url) become an
// ASAuthorizationPlatformPublicKeyCredentialProvider request; the credential
// comes back in the WebAuthn JSON form (`toJSON()`), for the server to
// verify. The relying party's domain must list the app (`webcredentials:`
// associated domain, apple-app-site-association).

#if os(watchOS)
@_cdecl("prism_webauthn")
public func prismWebAuthn(_ id: UInt64, _ create: Bool, _ options: UnsafePointer<CChar>) {
    failure(id, "unavailable") // no platform passkeys on the watch
}

@_cdecl("prism_passkeys_available")
public func prismPasskeysAvailable() -> Bool { false }
#else
private func base64url(_ s: String?) -> Data? {
    guard var s = s?.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/") else { return nil }
    while s.count % 4 != 0 { s += "=" }
    return Data(base64Encoded: s)
}

private func base64url(_ d: Data) -> String {
    d.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
}

private var passkeyRequests: [UInt64: PrismPasskeys] = [:]

final class PrismPasskeys: NSObject, ASAuthorizationControllerDelegate, ASAuthorizationControllerPresentationContextProviding {
    let id: UInt64
    var controller: ASAuthorizationController?
    init(id: UInt64) { self.id = id }

    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor { anchor() }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization) {
        defer { passkeyRequests[id] = nil }
        var json: [String: Any] = ["type": "public-key", "authenticatorAttachment": "platform", "clientExtensionResults": [String: Any]()]
        if let r = authorization.credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration {
            json["id"] = base64url(r.credentialID)
            json["rawId"] = base64url(r.credentialID)
            json["response"] = [
                "clientDataJSON": base64url(r.rawClientDataJSON),
                "attestationObject": base64url(r.rawAttestationObject ?? Data()),
                "transports": ["internal", "hybrid"],
            ]
        } else if let a = authorization.credential as? ASAuthorizationPlatformPublicKeyCredentialAssertion {
            json["id"] = base64url(a.credentialID)
            json["rawId"] = base64url(a.credentialID)
            json["response"] = [
                "clientDataJSON": base64url(a.rawClientDataJSON),
                "authenticatorData": base64url(a.rawAuthenticatorData),
                "signature": base64url(a.signature),
                "userHandle": base64url(a.userID),
            ]
        } else {
            return failure(id, "unexpected credential")
        }
        guard let data = try? JSONSerialization.data(withJSONObject: json) else { return failure(id, "unreadable credential") }
        text(id, String(decoding: data, as: UTF8.self))
    }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
        defer { passkeyRequests[id] = nil }
        switch (error as? ASAuthorizationError)?.code {
        case .canceled: failure(id, "cancelled")
        case .notHandled, .unknown:
            failure(id, "passkeys failed: the relying party's domain must list this app (a webcredentials: associated domain) (\(error.localizedDescription))")
        default: failure(id, error.localizedDescription)
        }
    }
}

private func userVerification(_ o: [String: Any]?) -> ASAuthorizationPublicKeyCredentialUserVerificationPreference {
    switch o?["userVerification"] as? String {
    case "required": return .required
    case "discouraged": return .discouraged
    default: return .preferred
    }
}

@_cdecl("prism_webauthn")
public func prismWebAuthn(_ id: UInt64, _ create: Bool, _ options: UnsafePointer<CChar>) {
    guard let o = (try? JSONSerialization.jsonObject(with: Data(String(cString: options).utf8))) as? [String: Any],
          let challenge = base64url(o["challenge"] as? String)
    else { return failure(id, "the WebAuthn options have no challenge") }
    let rp = create ? (o["rp"] as? [String: Any])?["id"] as? String : o["rpId"] as? String
    guard let rp, !rp.isEmpty else { return failure(id, "the WebAuthn options name no relying party id") }
    let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: rp)
    let request: ASAuthorizationRequest
    if create {
        guard let user = o["user"] as? [String: Any], let userID = base64url(user["id"] as? String),
              let name = user["name"] as? String
        else { return failure(id, "the WebAuthn options have no user") }
        let r = provider.createCredentialRegistrationRequest(challenge: challenge, name: name, userID: userID)
        if let shown = user["displayName"] as? String, !shown.isEmpty { r.displayName = shown }
        r.userVerificationPreference = userVerification(o["authenticatorSelection"] as? [String: Any])
        #if !os(tvOS) // tvOS has no attestation preference
        switch o["attestation"] as? String {
        case "direct": r.attestationPreference = .direct
        case "indirect": r.attestationPreference = .indirect
        case "enterprise": r.attestationPreference = .enterprise
        default: r.attestationPreference = .none
        }
        #endif
        if #available(iOS 17.4, macOS 14.4, tvOS 17.4, visionOS 1.1, *) {
            r.excludedCredentials = ((o["excludeCredentials"] as? [[String: Any]]) ?? []).compactMap {
                base64url($0["id"] as? String).map { ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: $0) }
            }
        }
        request = r
    } else {
        let r = provider.createCredentialAssertionRequest(challenge: challenge)
        r.userVerificationPreference = userVerification(o)
        r.allowedCredentials = ((o["allowCredentials"] as? [[String: Any]]) ?? []).compactMap {
            base64url($0["id"] as? String).map { ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: $0) }
        }
        request = r
    }
    let delegate = PrismPasskeys(id: id)
    let controller = ASAuthorizationController(authorizationRequests: [request])
    controller.delegate = delegate
    controller.presentationContextProvider = delegate
    delegate.controller = controller
    passkeyRequests[id] = delegate
    controller.performRequests()
}

@_cdecl("prism_passkeys_available")
public func prismPasskeysAvailable() -> Bool { true }
#endif

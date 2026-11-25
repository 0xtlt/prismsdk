// In-app purchases for the Rust app (prism_core::store, #306): StoreKit 2.
//
// Products, purchases, what the user owns (verified transactions of
// `Transaction.currentEntitlements`) and its changes (`Transaction.updates`:
// a purchase on another device, a refund, an approved Ask to Buy).
// StoreKit 2 is on every platform Prism builds for (iOS 15, macOS 12,
// tvOS 15, watchOS 8, visionOS 1); `purchase(confirmIn:)` needs a scene on
// iOS, tvOS and visionOS 17, a window on macOS 15.2.
import Foundation
import StoreKit
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
/// What the user owns now (a JSON array of product ids), for a watch.
@_silgen_name("prism_store_event")
private func storeEvent(_ id: UInt64, _ json: UnsafePointer<CChar>)

private func text(_ id: UInt64, _ value: String) {
    DispatchQueue.main.async { value.withCString { replyText(id, $0) } }
}

private func failure(_ id: UInt64, _ message: String) {
    DispatchQueue.main.async { message.withCString { replyError(id, false, $0) } }
}

private func json(_ value: Any) -> String {
    let data = (try? JSONSerialization.data(withJSONObject: value)) ?? Data("[]".utf8)
    return String(decoding: data, as: UTF8.self)
}

/// `cancelled` and `denied` as Rust reads them (`DeviceError::from_message`),
/// StoreKit's named errors as `DeviceError::Platform` (`networkError`…),
/// else the error's message.
private func message(_ error: Error) -> String {
    let named = { (name: String) in "\(name)\u{1f}\(error.localizedDescription)" }
    if let e = error as? StoreKitError {
        switch e {
        case .userCancelled: return "cancelled"
        case .networkError: return named("networkError")
        case .systemError: return named("systemError")
        case .notAvailableInStorefront: return named("notAvailableInStorefront")
        case .unknown: return named("unknown")
        default: return error.localizedDescription
        }
    }
    if let e = error as? Product.PurchaseError {
        switch e {
        case .purchaseNotAllowed: return "denied"
        case .productUnavailable: return named("productUnavailable")
        default: return error.localizedDescription
        }
    }
    return error.localizedDescription
}

// MARK: What the user owns

enum PrismPurchases {
    /// Entitlement streams of the app (main thread).
    static var watchers = Set<UInt64>()
    /// Bumped by each change: only the latest answer reaches the app.
    static var generation = 0
    static var listener: Task<Void, Never>?

    /// The ids of the products the user owns: verified, not revoked.
    static func owned() async -> [String] {
        var ids = Set<String>()
        for await result in Transaction.currentEntitlements {
            if case .verified(let t) = result, t.revocationDate == nil {
                ids.insert(t.productID)
            }
        }
        return ids.sorted()
    }

    /// Listens to StoreKit's transactions for the rest of the run (main
    /// thread). Transactions left unfinished by an earlier run arrive first.
    static func listen() {
        guard listener == nil else { return }
        listener = Task.detached {
            for await result in Transaction.unfinished {
                if case .verified(let t) = result { await t.finish() }
            }
            changed()
            for await result in Transaction.updates {
                // An unverified transaction grants nothing: left unfinished.
                if case .verified(let t) = result { await t.finish() }
                changed()
            }
        }
    }

    /// Tells every watch what the user owns now.
    static func changed() {
        DispatchQueue.main.async {
            generation += 1
            let mine = generation
            Task.detached {
                let ids = json(await owned())
                DispatchQueue.main.async {
                    guard mine == generation else { return }
                    for id in watchers { ids.withCString { storeEvent(id, $0) } }
                }
            }
        }
    }

    /// Stops stream `id` (prism_device_stop).
    static func stop(_ id: UInt64) {
        watchers.remove(id)
    }
}

@_cdecl("prism_store_watch")
public func prismStoreWatch(_ id: UInt64) {
    DispatchQueue.main.async {
        PrismPurchases.watchers.insert(id)
        PrismPurchases.listen()
        PrismPurchases.changed()
    }
}

/// What the user owns, after `AppStore.sync()` when `restore`.
@_cdecl("prism_store_entitlements")
public func prismStoreEntitlements(_ id: UInt64, _ restore: Bool) {
    Task.detached {
        if restore {
            do { try await AppStore.sync() } catch {
                failure(id, message(error))
                return
            }
        }
        text(id, json(await PrismPurchases.owned()))
        PrismPurchases.changed()
    }
}

// MARK: Products

@_cdecl("prism_store_products")
public func prismStoreProducts(_ id: UInt64, _ ids: UnsafePointer<CChar>) {
    let wanted = String(cString: ids).split(separator: "\n").map(String.init)
    Task.detached {
        do {
            let products = try await Product.products(for: wanted)
            let list: [[String: Any]] = products.map { p in
                [
                    "id": p.id,
                    "name": p.displayName,
                    "description": p.description,
                    "price": p.displayPrice,
                    "micros": NSDecimalNumber(decimal: p.price).multiplying(byPowerOf10: 6).int64Value,
                    "currency": p.priceFormatStyle.currencyCode,
                ]
            }
            text(id, json(list))
        } catch {
            failure(id, message(error))
        }
    }
}

// MARK: Purchases

/// Shows StoreKit's sheet over the app's active scene or window.
@MainActor
private func buy(_ product: Product, _ options: Set<Product.PurchaseOption>) async throws -> Product.PurchaseResult {
    #if os(iOS) || os(tvOS) || os(visionOS)
    let scenes = UIApplication.shared.connectedScenes
    if let scene = scenes.first(where: { $0.activationState == .foregroundActive }) ?? scenes.first {
        return try await product.purchase(confirmIn: scene, options: options)
    }
    #if os(visionOS)
    throw StoreKitError.unknown
    #else
    return try await product.purchase(options: options)
    #endif
    #elseif os(macOS)
    if #available(macOS 15.2, *), let window = NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first {
        return try await product.purchase(confirmIn: window, options: options)
    }
    return try await product.purchase(options: options)
    #else
    return try await product.purchase(options: options)
    #endif
}

@_cdecl("prism_store_purchase")
public func prismStorePurchase(_ id: UInt64, _ product: UnsafePointer<CChar>, _ account: UnsafePointer<CChar>?) {
    let productID = String(cString: product)
    let account = account.map { String(cString: $0) }
    Task { @MainActor in
        guard AppStore.canMakePayments else { return failure(id, "denied") }
        var options = Set<Product.PurchaseOption>()
        if let account {
            guard let uuid = UUID(uuidString: account) else {
                return failure(id, "the account token \(account) is not a UUID")
            }
            options.insert(.appAccountToken(uuid))
        }
        do {
            guard let found = try await Product.products(for: [productID]).first else {
                return failure(id, "the App Store has no product \(productID) for this app")
            }
            switch try await buy(found, options) {
            case .success(.verified(let transaction)):
                await transaction.finish()
                text(id, "purchased")
                PrismPurchases.changed()
            case .success(.unverified(_, let error)):
                failure(id, "the App Store's transaction did not verify: \(error.localizedDescription)")
            case .pending:
                text(id, "pending")
            case .userCancelled:
                failure(id, "cancelled")
            @unknown default:
                failure(id, "StoreKit answered a purchase result Prism does not know")
            }
        } catch {
            failure(id, message(error))
        }
    }
}

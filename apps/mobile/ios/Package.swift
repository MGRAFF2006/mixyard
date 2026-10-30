// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "MixyardMobileCore", platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(name: "MobileCore", targets: ["MobileCore"]),
        .library(name: "MobileControlTrust", type: .static, targets: ["MobileControlTrust"])
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-asn1.git", revision: "9f542610331815e29cc3821d3b6f488db8715517")
    ],
    targets: [
        .target(name: "MobileCore"),
        .target(name: "MobileControlTrust", dependencies: [.product(name: "SwiftASN1", package: "swift-asn1")]),
        .testTarget(name: "MobileCoreTests", dependencies: ["MobileCore"]),
        .testTarget(name: "MobileControlTrustTests", dependencies: ["MobileControlTrust"])
    ])

// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "MixyardMobileCore", platforms: [.iOS(.v16), .macOS(.v13)],
    products: [.library(name: "MobileCore", targets: ["MobileCore"])],
    targets: [.target(name: "MobileCore"), .testTarget(name: "MobileCoreTests", dependencies: ["MobileCore"])])

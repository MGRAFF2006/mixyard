import XCTest

@MainActor final class ConfirmationUITests: XCTestCase {
    func testActualSharedDirtyReplacementAndCodecRemovalUseNativeDialogs() {
        continueAfterFailure = false
        let app = XCUIApplication(); app.launch()
        let web = app.webViews.firstMatch
        XCTAssertTrue(web.waitForExistence(timeout: 10))
        let observed = web.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Observed gain=")).firstMatch
        func wait(_ text: String) {
            let expectation = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label CONTAINS %@", text), object: observed)
            XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: 10), .completed)
        }
        func click(_ label: String) { web.buttons[label].firstMatch.tap() }
        func answer(_ label: String) {
            let alert = app.alerts["Confirm change"]
            XCTAssertTrue(alert.waitForExistence(timeout: 10), "Missing native confirmation")
            alert.buttons[label].tap()
        }
        click("Fixture load example")
        let loaded = XCTNSPredicateExpectation(predicate: NSPredicate(format: "NOT (label CONTAINS %@)", "gain=none"), object: observed)
        XCTAssertEqual(XCTWaiter.wait(for: [loaded], timeout: 10), .completed)
        let baseline = observed.label
        click("Fixture edit gain")
        wait("gain=0.37")
        click("Fixture load example"); answer("Cancel"); wait("gain=0.37")
        click("Fixture load example"); answer("Continue")
        let reset = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label == %@", baseline), object: observed)
        XCTAssertEqual(XCTWaiter.wait(for: [reset], timeout: 10), .completed)
        click("Fixture codecs"); wait("drafts=1")
        click("Fixture add codec"); wait("drafts=2")
        click("Fixture edit codec")
        click("Fixture remove codec"); answer("Cancel"); wait("drafts=2")
        click("Fixture remove codec"); answer("Continue"); wait("drafts=1")
        click("Fixture routing"); click("Fixture load example")
        XCTAssertTrue(app.alerts["Confirm change"].waitForExistence(timeout: 10))
        XCUIDevice.shared.press(.home); app.activate()
        XCTAssertFalse(app.alerts["Confirm change"].exists, "Background must cancel pending confirmation")
        XCTAssertTrue(observed.exists)
    }
}

# In the car (CarPlay, Android Auto, Android Automotive)

Cars only show the platform's templates, so a car screen is described with them: lists in sections, grids, panes (rows and up to two actions) and tabs. They are rebuilt whenever the app changes, like widgets; the car only receives what differs.

```rust
cx.register_car(move |cx| {
    CarTemplate::list("Todos").section(CarSection::new().items(
        store.read(cx).todos.iter().map(|t| {
            let id = t.id;
            CarItem::new(&t.title).icon("circle").browsable()
                .on_select(move |cx| cx.car_push(move |cx| detail(cx, id)))
        }),
    ))
});
// In a handler: cx.car_push(…), cx.car_pop(), cx.car_pop_to_root(); cx.car_connected().
```

- **CarPlay:**
  - a template scene (`CPListTemplate`, `CPGridTemplate`, `CPInformationTemplate`, `CPTabBarTemplate`), updated in place;
  - the car can start the app without its phone window;
  - CarPlay only shows apps whose provisioning profile carries an entitlement Apple grants on request (`com.apple.developer.carplay-driving-task`), even in the simulator. Builds declare the scene; the entitlement comes from your profile.
- **Android Auto and Android Automotive:**
  - Google's Car App Library (IoT category), fetched from Maven by `crates/prism-cli/platform/scripts/android-deps.py` (no Gradle);
  - the phone APK serves Android Auto. A build for a car running Android (`PRISM_ANDROID_AUTOMOTIVE=1`, automatic when the device is one) opens the car screens from the car's launcher;
  - tabs show their first tab (`TabTemplate` needs Car API level 6);
  - test on the Automotive emulator (`system-images;android-35-ext15;android-automotive;arm64-v8a`, device `automotive_1408p_landscape_with_google_apis`), or with Android Auto's Desktop Head Unit and a phone signed in to Play.

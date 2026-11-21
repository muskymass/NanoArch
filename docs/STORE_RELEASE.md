# NanoArch: Microsoft Store release

Status of the packaging work and what only the publisher can do. Sources were read on 2026-11-21 from Microsoft Learn
(pages and their own "updated" dates are named below). Anything not read from a source is marked **not verified**.

## Built and checked here
| Item | State |
|------|-------|
| MSIX package (`store/build-msix.ps1`, `store/AppxManifest.xml`) | Builds unsigned with `makeappx pack`; unpacks back with all expected files. Not installed or run from the package. |
| Version | Taken from `Cargo.toml` (`x.y.z` becomes `x.y.z.0`). The Store reserves the 4th part and it must be 0; the 1st part cannot be 0 (app-package-requirements, updated 2026-08-24). The script refuses a version starting with 0. |
| Signing | Not signed on purpose: the Store re-signs MSIX packages (same page). The Inno Setup installer stays unsigned and is not for the Store. |
| Capability | `runFullTrust` (a Win32 exe). Restricted capability: Partner Center asks for a description at submission and a testers' denial fails certification (app-capability-declarations, updated 2026-11-12). Draft justification below. |
| Network | The app opens no outside connection. The one network client is the optional brief assistant, which only connects to `localhost`, `127.0.0.1` or `::1` (refused otherwise, `src/ollama.rs`). The page loads no external URL. |
| Writable data | The app writes only where the user picks (Save dialogs). WebView2's data folder is wry's default under `%LOCALAPPDATA%`, which is writable for a packaged app. **Not verified from the package**, only by reading the code. |

## Only the publisher can do these
1. **Partner Center account** and reserving the app name. I cannot create accounts.
2. **Product identity**: copy `Package/Identity/Name`, `Package/Identity/Publisher` (the full `CN=...`) and the
   publisher display name, then build:
   ```
   .\store\build-msix.ps1 -IdentityName "<Name>" -Publisher "CN=<...>" -PublisherDisplayName "<display name>"
   ```
   The output is `dist\NanoArch-<version>.msix`. Upload that file.
3. **Privacy policy URL**: a packaged Win32 product must have one (Store Policies 7.20, section 10.5.1). A draft is in
   `docs/PRIVACY_POLICY.md`: host it at a public URL, fill the contact line, and have it read by whoever is responsible.
4. **Age rating**: complete the IARC questionnaire in Partner Center (section 11.11, from a model summary, not verbatim).
5. **Pricing, markets, languages** and the store listing text (draft below).
6. **Screenshots**: five captured from the real app at 1920 x 1080 in `store/screenshots` (brief, plan, quantities, BOQ, 3D),
   all with the app's synthetic example data. Store size and count rules were **not verified**; review them before upload.
7. **Certification run** of the Windows App Certification Kit on the installed package: not run here.

## Known risks to settle before submitting
- **WebView2 runtime.** Windows 11 includes it; "a small number" of Windows 10 devices do not (WebView2 distribution page,
  updated 2026-10-02). Found by simulation on 2026-11-21 (`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` pointing at an empty
  folder, Windows 11): the app used to panic with exit code 101 and no message. It now shows an English/Arabic dialog
  naming the runtime and its download page, then exits. **Not tested** on a real Windows 10 machine without the runtime
  (no VM available here: Windows 11 Home has no Sandbox or Hyper-V), nor from the installed MSIX package.
- **Minimum Windows.** The manifest says 10.0.19041.0 (first build that accepts `uap10:RuntimeBehavior`, per the manual
  conversion page, updated 2026-04-15). No older or other Windows build was tested; the app was run on Windows 11 only.
- **Logo files.** The Learn page for logo sizes returned 404, so the sizes used (44, 150, 50, 310x150) are the commonly
  used ones, **not verified**, and the images are the app icon scaled with no padding or dark variants.
- **Architecture.** x64 only.
- **Preliminary output.** The app does not state compliance with the Saudi Building Code: values that were not read from
  a source stay unset and their rules are reported as skipped. The listing must not promise compliance.
- **Templates folder.** The Inno installer copies empty code-rule and rates templates for the user to fill. The package
  does not (its install folder is read-only); they are also available from the repository (`nnx-core/packs`).

## Draft justification for `runFullTrust` (English)
> NanoArch is a Win32 desktop application written in Rust. It opens and saves the user's project files and exports
> (IFC, DXF, PDF, XLSX, CSV) in locations the user chooses with the standard file dialogs, and hosts its interface in the
> Microsoft Edge WebView2 runtime. It needs the full-trust desktop runtime because it is not a UWP app.

## Draft listing (review before use)
**English**
- Name: NanoArch
- Short description: Preliminary residential plans, quantities and BOQ from one project model.
- Description: NanoArch turns a room program and a plot into alternative villa floor plans, then derives drawings, quantity
  take-off, bill of quantities (with your own rates and specifications), structural and MEP estimates and a schedule
  from the same model. Import a draft image or PDF to measure the plot and rooms. Export to IFC, DXF, PDF, Excel and CSV.
  Code values, prices and productivity rates are entered by you with their sources; anything missing is listed by name
  and totals are marked partial. Output is preliminary and does not replace a licensed engineer or the approving authority.

**العربية**
- الاسم: NanoArch
- وصف قصير: مخططات سكنية أولية وكميات وجدول كميات من نموذج مشروع واحد.
- الوصف: يحوّل NanoArch برنامج الغرف وأبعاد الأرض إلى بدائل لمخطط فيلا، ثم يستخرج من النموذج نفسه الرسومات وحصر الكميات
  وجدول الكميات (بأسعارك ومواصفاتك) وتقديرات إنشائية وكهروميكانيكية وجدولاً زمنياً. يمكنك استيراد رسم أولي (صورة أو PDF)
  لقياس الأرض والغرف. التصدير إلى IFC وDXF وPDF وExcel وCSV. قيم الكود والأسعار ومعدلات الإنتاج تُدخلها أنت مع مصدرها،
  وما يفقد منها يُذكر بالاسم وتُعلَّم الإجماليات بأنها جزئية. المخرجات أولية ولا تغني عن مهندس مرخّص أو عن الجهة المعتمدة.

## Sources read
- Manual MSIX packaging of a Win32 exe, updated 2026-04-15: `learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-manual-conversion`
- Store package requirements, updated 2026-08-24: `learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/app-package-requirements`
- App capability declarations, updated 2026-11-12: `learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations`
- WebView2 distribution, updated 2026-10-02: `learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution`
- Microsoft Store Policies 7.20, updated 2026-09-15: `learn.microsoft.com/en-us/windows/apps/publish/store-policies`

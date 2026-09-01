; ============================================================
; ZViewer NSIS 安装器自定义钩子（bundle.windows.nsis.installerHooks）
;
; 功能（仅 Windows，仅注册到 HKCU\Software\Classes，per-user，无需管理员）：
;   1. 非抢占式文件关联：只把 ZViewer 注册为「打开方式」候选
;      （OpenWithProgids + Applications + ProgId），不修改任何扩展名的
;      默认打开程序 → Windows 设置 > 应用 > 默认应用 > 照片查看器
;      中可看到并选择 ZViewer，双击图片的默认行为不变。
;   2. 图片文件右键菜单：「用 ZViewer 打开」（Win11 在
;      「显示更多选项」中）。仅对图片/RAW 扩展名注册。
;   3. App Capabilities（HKCU\Software\RegisteredApplications）：
;      让 ZViewer 以应用名出现在 Windows 默认应用搜索/列表里。
;
; 扩展名清单与 src/browse/mod.rs COMMON_EXTS + src/decode/mod.rs
; RAW_EXTS 保持一致，修改图片格式支持时需同步此处。
; ============================================================

!define ZVIEWER_PROGID "ZViewer.Image"
!define ZVIEWER_PROGID_DESC "Image file"
; verb 名用独特命名空间，避免与同扩展名下其他软件注册的 verb 撞名（卸载时才不会误删他人条目）
!define ZVIEWER_VERB "ZViewer.Open"
!define ZVIEWER_VERB_TEXT "用 ZViewer 打开"
!define ZVIEWER_CAPABILITIES "Software\ZViewer\Capabilities"

; ------------------------------------------------------------------
; 注册单个扩展名：ProgId（打开方式/默认应用候选）+ 右键菜单 verb
; ------------------------------------------------------------------
!macro ZViewerRegisterExt EXT
  WriteRegStr HKCU "Software\Classes\${ZVIEWER_PROGID}" "" "${ZVIEWER_PROGID_DESC}"
  WriteRegStr HKCU "Software\Classes\${ZVIEWER_PROGID}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr HKCU "Software\Classes\${ZVIEWER_PROGID}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\.${EXT}\OpenWithProgids" "${ZVIEWER_PROGID}" ""
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${ZVIEWER_VERB}" "" "${ZVIEWER_VERB_TEXT}"
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${ZVIEWER_VERB}\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${ZVIEWER_VERB}\Icon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
!macroend

; ------------------------------------------------------------------
; 移除单个扩展名的注册（幂等；/ifempty 保护可能被其他程序共用的键）
; ------------------------------------------------------------------
!macro ZViewerUnregisterExt EXT
  DeleteRegValue HKCU "Software\Classes\.${EXT}\OpenWithProgids" "${ZVIEWER_PROGID}"
  DeleteRegKey /ifempty HKCU "Software\Classes\.${EXT}\OpenWithProgids"
  DeleteRegKey HKCU "Software\Classes\.${EXT}\shell\${ZVIEWER_VERB}"
  DeleteRegKey /ifempty HKCU "Software\Classes\.${EXT}\shell"
!macroend

; ------------------------------------------------------------------
; 安装后：注册全部扩展名
; ------------------------------------------------------------------
!macro NSIS_HOOK_POSTINSTALL
  ; ProgId 与右键菜单（常见格式）
  !insertmacro ZViewerRegisterExt "jpg"
  !insertmacro ZViewerRegisterExt "jpeg"
  !insertmacro ZViewerRegisterExt "png"
  !insertmacro ZViewerRegisterExt "gif"
  !insertmacro ZViewerRegisterExt "webp"
  !insertmacro ZViewerRegisterExt "bmp"
  !insertmacro ZViewerRegisterExt "ico"
  !insertmacro ZViewerRegisterExt "svg"
  ; ProgId 与右键菜单（相机 RAW）
  !insertmacro ZViewerRegisterExt "cr2"
  !insertmacro ZViewerRegisterExt "cr3"
  !insertmacro ZViewerRegisterExt "nef"
  !insertmacro ZViewerRegisterExt "arw"
  !insertmacro ZViewerRegisterExt "dng"
  !insertmacro ZViewerRegisterExt "orf"
  !insertmacro ZViewerRegisterExt "rw2"
  !insertmacro ZViewerRegisterExt "pef"
  !insertmacro ZViewerRegisterExt "srw"
  !insertmacro ZViewerRegisterExt "raf"
  !insertmacro ZViewerRegisterExt "raw"
  !insertmacro ZViewerRegisterExt "x3f"
  !insertmacro ZViewerRegisterExt "erf"
  !insertmacro ZViewerRegisterExt "3fr"
  !insertmacro ZViewerRegisterExt "kdc"
  !insertmacro ZViewerRegisterExt "dcr"
  !insertmacro ZViewerRegisterExt "mrw"
  !insertmacro ZViewerRegisterExt "mef"
  !insertmacro ZViewerRegisterExt "mos"
  !insertmacro ZViewerRegisterExt "iiq"
  !insertmacro ZViewerRegisterExt "fff"
  !insertmacro ZViewerRegisterExt "ari"

  ; 「打开方式 > 选择其他应用」列表条目（以 exe 名注册）
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"

  ; App Capabilities：出现在 Windows 默认应用（按应用查看）列表
  WriteRegStr HKCU "Software\RegisteredApplications" "${PRODUCTNAME}" "${ZVIEWER_CAPABILITIES}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}" "ApplicationName" "${PRODUCTNAME}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}" "ApplicationDescription" "精美快速的看图软件"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".jpg" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".jpeg" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".png" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".gif" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".webp" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".bmp" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".ico" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".svg" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".cr2" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".cr3" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".nef" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".arw" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".dng" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".orf" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".rw2" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".pef" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".srw" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".raf" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".raw" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".x3f" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".erf" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".3fr" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".kdc" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".dcr" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".mrw" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".mef" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".mos" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".iiq" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".fff" "${ZVIEWER_PROGID}"
  WriteRegStr HKCU "${ZVIEWER_CAPABILITIES}\FileAssociations" ".ari" "${ZVIEWER_PROGID}"

  ; 通知资源管理器刷新关联/图标缓存
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend

; ------------------------------------------------------------------
; 卸载后：清理全部注册（只删本应用写入的键）
; ------------------------------------------------------------------
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro ZViewerUnregisterExt "jpg"
  !insertmacro ZViewerUnregisterExt "jpeg"
  !insertmacro ZViewerUnregisterExt "png"
  !insertmacro ZViewerUnregisterExt "gif"
  !insertmacro ZViewerUnregisterExt "webp"
  !insertmacro ZViewerUnregisterExt "bmp"
  !insertmacro ZViewerUnregisterExt "ico"
  !insertmacro ZViewerUnregisterExt "svg"
  !insertmacro ZViewerUnregisterExt "cr2"
  !insertmacro ZViewerUnregisterExt "cr3"
  !insertmacro ZViewerUnregisterExt "nef"
  !insertmacro ZViewerUnregisterExt "arw"
  !insertmacro ZViewerUnregisterExt "dng"
  !insertmacro ZViewerUnregisterExt "orf"
  !insertmacro ZViewerUnregisterExt "rw2"
  !insertmacro ZViewerUnregisterExt "pef"
  !insertmacro ZViewerUnregisterExt "srw"
  !insertmacro ZViewerUnregisterExt "raf"
  !insertmacro ZViewerUnregisterExt "raw"
  !insertmacro ZViewerUnregisterExt "x3f"
  !insertmacro ZViewerUnregisterExt "erf"
  !insertmacro ZViewerUnregisterExt "3fr"
  !insertmacro ZViewerUnregisterExt "kdc"
  !insertmacro ZViewerUnregisterExt "dcr"
  !insertmacro ZViewerUnregisterExt "mrw"
  !insertmacro ZViewerUnregisterExt "mef"
  !insertmacro ZViewerUnregisterExt "mos"
  !insertmacro ZViewerUnregisterExt "iiq"
  !insertmacro ZViewerUnregisterExt "fff"
  !insertmacro ZViewerUnregisterExt "ari"

  ; Applications 条目与 ProgId（只删本应用创建的键）
  DeleteRegKey HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  DeleteRegKey HKCU "Software\Classes\${ZVIEWER_PROGID}"

  ; App Capabilities（RegisteredApplications 值 + Capabilities 键）
  DeleteRegValue HKCU "Software\RegisteredApplications" "${PRODUCTNAME}"
  DeleteRegKey HKCU "${ZVIEWER_CAPABILITIES}"
  DeleteRegKey /ifempty HKCU "Software\ZViewer"
  DeleteRegKey /ifempty HKCU "Software\RegisteredApplications"

  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend

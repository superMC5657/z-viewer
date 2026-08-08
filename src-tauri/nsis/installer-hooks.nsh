; ============================================================
; Image Viewer NSIS 安装器自定义钩子（bundle.windows.nsis.installerHooks）
;
; 功能（仅 Windows，仅注册到 HKCU\Software\Classes，per-user，无需管理员）：
;   1. 非抢占式文件关联：只把 Image Viewer 注册为「打开方式」候选
;      （OpenWithProgids + Applications + ProgId），不修改任何扩展名的
;      默认打开程序 → Windows 设置 > 应用 > 默认应用 > 照片查看器
;      中可看到并选择 Image Viewer，双击图片的默认行为不变。
;   2. 图片文件右键菜单：「用 Image Viewer 打开」（Win11 在
;      「显示更多选项」中）。仅对图片/RAW 扩展名注册。
;   3. App Capabilities（HKCU\Software\RegisteredApplications）：
;      让 Image Viewer 以应用名出现在 Windows 默认应用搜索/列表里。
;
; 扩展名清单与 src/browse/mod.rs COMMON_EXTS + src/decode/mod.rs
; RAW_EXTS 保持一致，修改图片格式支持时需同步此处。
; ============================================================

!define IMAGEVIEWER_PROGID "ImageViewer.Image"
!define IMAGEVIEWER_PROGID_DESC "Image file"
; verb 名用独特命名空间，避免与同扩展名下其他软件注册的 verb 撞名（卸载时才不会误删他人条目）
!define IMAGEVIEWER_VERB "ImageViewer.Open"
!define IMAGEVIEWER_VERB_TEXT "用 Image Viewer 打开"
!define IMAGEVIEWER_CAPABILITIES "Software\ImageViewer\Capabilities"

; ------------------------------------------------------------------
; 注册单个扩展名：ProgId（打开方式/默认应用候选）+ 右键菜单 verb
; ------------------------------------------------------------------
!macro ImageViewerRegisterExt EXT
  WriteRegStr HKCU "Software\Classes\${IMAGEVIEWER_PROGID}" "" "${IMAGEVIEWER_PROGID_DESC}"
  WriteRegStr HKCU "Software\Classes\${IMAGEVIEWER_PROGID}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
  WriteRegStr HKCU "Software\Classes\${IMAGEVIEWER_PROGID}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\.${EXT}\OpenWithProgids" "${IMAGEVIEWER_PROGID}" ""
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${IMAGEVIEWER_VERB}" "" "${IMAGEVIEWER_VERB_TEXT}"
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${IMAGEVIEWER_VERB}\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\.${EXT}\shell\${IMAGEVIEWER_VERB}\Icon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
!macroend

; ------------------------------------------------------------------
; 移除单个扩展名的注册（幂等；/ifempty 保护可能被其他程序共用的键）
; ------------------------------------------------------------------
!macro ImageViewerUnregisterExt EXT
  DeleteRegValue HKCU "Software\Classes\.${EXT}\OpenWithProgids" "${IMAGEVIEWER_PROGID}"
  DeleteRegKey /ifempty HKCU "Software\Classes\.${EXT}\OpenWithProgids"
  DeleteRegKey HKCU "Software\Classes\.${EXT}\shell\${IMAGEVIEWER_VERB}"
  DeleteRegKey /ifempty HKCU "Software\Classes\.${EXT}\shell"
!macroend

; ------------------------------------------------------------------
; 安装后：注册全部扩展名
; ------------------------------------------------------------------
!macro NSIS_HOOK_POSTINSTALL
  ; ProgId 与右键菜单（常见格式）
  !insertmacro ImageViewerRegisterExt "jpg"
  !insertmacro ImageViewerRegisterExt "jpeg"
  !insertmacro ImageViewerRegisterExt "png"
  !insertmacro ImageViewerRegisterExt "gif"
  !insertmacro ImageViewerRegisterExt "webp"
  !insertmacro ImageViewerRegisterExt "bmp"
  !insertmacro ImageViewerRegisterExt "ico"
  !insertmacro ImageViewerRegisterExt "svg"
  ; ProgId 与右键菜单（相机 RAW）
  !insertmacro ImageViewerRegisterExt "cr2"
  !insertmacro ImageViewerRegisterExt "cr3"
  !insertmacro ImageViewerRegisterExt "nef"
  !insertmacro ImageViewerRegisterExt "arw"
  !insertmacro ImageViewerRegisterExt "dng"
  !insertmacro ImageViewerRegisterExt "orf"
  !insertmacro ImageViewerRegisterExt "rw2"
  !insertmacro ImageViewerRegisterExt "pef"
  !insertmacro ImageViewerRegisterExt "srw"
  !insertmacro ImageViewerRegisterExt "raf"
  !insertmacro ImageViewerRegisterExt "raw"
  !insertmacro ImageViewerRegisterExt "x3f"
  !insertmacro ImageViewerRegisterExt "erf"
  !insertmacro ImageViewerRegisterExt "3fr"
  !insertmacro ImageViewerRegisterExt "kdc"
  !insertmacro ImageViewerRegisterExt "dcr"
  !insertmacro ImageViewerRegisterExt "mrw"
  !insertmacro ImageViewerRegisterExt "mef"
  !insertmacro ImageViewerRegisterExt "mos"
  !insertmacro ImageViewerRegisterExt "iiq"
  !insertmacro ImageViewerRegisterExt "fff"
  !insertmacro ImageViewerRegisterExt "ari"

  ; 「打开方式 > 选择其他应用」列表条目（以 exe 名注册）
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  WriteRegStr HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"

  ; App Capabilities：出现在 Windows 默认应用（按应用查看）列表
  WriteRegStr HKCU "Software\RegisteredApplications" "${PRODUCTNAME}" "${IMAGEVIEWER_CAPABILITIES}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}" "ApplicationName" "${PRODUCTNAME}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}" "ApplicationDescription" "精美快速的看图软件"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".jpg" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".jpeg" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".png" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".gif" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".webp" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".bmp" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".ico" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".svg" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".cr2" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".cr3" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".nef" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".arw" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".dng" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".orf" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".rw2" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".pef" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".srw" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".raf" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".raw" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".x3f" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".erf" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".3fr" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".kdc" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".dcr" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".mrw" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".mef" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".mos" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".iiq" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".fff" "${IMAGEVIEWER_PROGID}"
  WriteRegStr HKCU "${IMAGEVIEWER_CAPABILITIES}\FileAssociations" ".ari" "${IMAGEVIEWER_PROGID}"

  ; 通知资源管理器刷新关联/图标缓存
  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend

; ------------------------------------------------------------------
; 卸载后：清理全部注册（只删本应用写入的键）
; ------------------------------------------------------------------
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro ImageViewerUnregisterExt "jpg"
  !insertmacro ImageViewerUnregisterExt "jpeg"
  !insertmacro ImageViewerUnregisterExt "png"
  !insertmacro ImageViewerUnregisterExt "gif"
  !insertmacro ImageViewerUnregisterExt "webp"
  !insertmacro ImageViewerUnregisterExt "bmp"
  !insertmacro ImageViewerUnregisterExt "ico"
  !insertmacro ImageViewerUnregisterExt "svg"
  !insertmacro ImageViewerUnregisterExt "cr2"
  !insertmacro ImageViewerUnregisterExt "cr3"
  !insertmacro ImageViewerUnregisterExt "nef"
  !insertmacro ImageViewerUnregisterExt "arw"
  !insertmacro ImageViewerUnregisterExt "dng"
  !insertmacro ImageViewerUnregisterExt "orf"
  !insertmacro ImageViewerUnregisterExt "rw2"
  !insertmacro ImageViewerUnregisterExt "pef"
  !insertmacro ImageViewerUnregisterExt "srw"
  !insertmacro ImageViewerUnregisterExt "raf"
  !insertmacro ImageViewerUnregisterExt "raw"
  !insertmacro ImageViewerUnregisterExt "x3f"
  !insertmacro ImageViewerUnregisterExt "erf"
  !insertmacro ImageViewerUnregisterExt "3fr"
  !insertmacro ImageViewerUnregisterExt "kdc"
  !insertmacro ImageViewerUnregisterExt "dcr"
  !insertmacro ImageViewerUnregisterExt "mrw"
  !insertmacro ImageViewerUnregisterExt "mef"
  !insertmacro ImageViewerUnregisterExt "mos"
  !insertmacro ImageViewerUnregisterExt "iiq"
  !insertmacro ImageViewerUnregisterExt "fff"
  !insertmacro ImageViewerUnregisterExt "ari"

  ; Applications 条目与 ProgId（只删本应用创建的键）
  DeleteRegKey HKCU "Software\Classes\Applications\${MAINBINARYNAME}.exe"
  DeleteRegKey HKCU "Software\Classes\${IMAGEVIEWER_PROGID}"

  ; App Capabilities（RegisteredApplications 值 + Capabilities 键）
  DeleteRegValue HKCU "Software\RegisteredApplications" "${PRODUCTNAME}"
  DeleteRegKey HKCU "${IMAGEVIEWER_CAPABILITIES}"
  DeleteRegKey /ifempty HKCU "Software\ImageViewer"
  DeleteRegKey /ifempty HKCU "Software\RegisteredApplications"

  System::Call "shell32::SHChangeNotify(i 0x08000000, i 0x1000, i 0, i 0)"
!macroend

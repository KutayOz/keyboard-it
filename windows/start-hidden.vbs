' win-receiver'ı KONSOL PENCERESİ AÇMADAN (gizli) başlatır.
' Oturum açılışı görevi bunu çalıştırır; exe yolunu bu script'in konumuna göre çözer,
' böylece klasör nereye kopyalanırsa kopyalansın çalışır.
'
' Kullanım (Görev Zamanlayıcı / schtasks):
'   wscript.exe "C:\...\keyboard-it\windows\start-hidden.vbs"

Set fso = CreateObject("Scripting.FileSystemObject")
scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)   ' ...\keyboard-it\windows
repoDir   = fso.GetParentFolderName(scriptDir)                ' ...\keyboard-it
exePath   = repoDir & "\target\release\win-receiver.exe"

If Not fso.FileExists(exePath) Then
    ' Release derlenmemiş: önce `cargo build --release -p win-receiver` gerekli.
    WScript.Quit 1
End If

' 0 = gizli pencere, False = bekleme. KEYBOARD_IT_KEY kullanıcı ortamından miras alınır.
CreateObject("WScript.Shell").Run """" & exePath & """", 0, False

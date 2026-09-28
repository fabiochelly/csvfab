@echo off
rem Lanceur Windows de csvfab : relaie vers le script Python csvfab.
rem pyw / pythonw plutot que python : pas de fenetre de console qui reste ouverte.
rem A associer aux .csv via "Ouvrir avec", ou a epingler tel quel.
where pyw >nul 2>nul && (start "" pyw "%~dp0csvfab" %* & exit /b)
where pythonw >nul 2>nul && (start "" pythonw "%~dp0csvfab" %* & exit /b)
python "%~dp0csvfab" %*

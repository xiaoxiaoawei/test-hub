// Package eda coordinates explicitly selected, locally installed EDA engines.
// Discovery is not proof of a valid license or a successful native conversion.
package eda

import "packagestudio/internal/core"

type Config struct {
	ModelAssociationMode string  `json:"modelAssociationMode"`
	PADSModelExe         string  `json:"padsModelExe"`
	PADSReuseDirectory   string  `json:"padsReuseDirectory"`
	Enabled              bool    `json:"enabled"`
	LibraryMode          string  `json:"libraryMode"`
	PADSRoot             string  `json:"padsRoot"`
	PADSSymbolLibrary    string  `json:"padsSymbolLibrary"`
	PADSFootprintLibrary string  `json:"padsFootprintLibrary"`
	CaptureExe           string  `json:"captureExe"`
	AllegroExe           string  `json:"allegroExe"`
	AllegroProduct       string  `json:"allegroProduct"`
	CaptureLibrary       string  `json:"captureLibrary"`
	AllegroDirectory     string  `json:"allegroDirectory"`
	CaptureUnitsPerInch  float64 `json:"captureUnitsPerInch"`
}

type Request struct {
	Parts            []core.Part
	OutputDirectory  string
	LibraryName      string
	IncludeSymbol    bool
	IncludeFootprint bool
	IncludeModel     bool
	Config           Config
}

type Installation struct {
	ID              string `json:"id"`
	Product         string `json:"product"`
	Root            string `json:"root"`
	Executable      string `json:"executable"`
	Version         string `json:"version"`
	Architecture    string `json:"architecture"`
	Source          string `json:"source"`
	Ready           bool   `json:"ready"`
	RuntimeVerified bool   `json:"runtimeVerified"`
	Message         string `json:"message,omitempty"`
}

type Environment struct {
	PADS     []Installation `json:"pads"`
	Cadence  []Installation `json:"cadence"`
	Warnings []string       `json:"warnings"`
}

// FileUpdate is published only after the selected engine has read back its staged
// output. An empty BeforeSHA256 requires an absent destination; nonempty means a
// pre-existing file whose content must still match the staging-time snapshot.
type FileUpdate struct {
	StagedPath   string
	Destination  string
	BeforeSHA256 string
}

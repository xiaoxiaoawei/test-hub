package service

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"packagestudio/internal/core"
)

type ExportHistoryEntry struct {
	ModelAssociations []core.ModelAssociation `json:"modelAssociations,omitempty"`
	ArtifactType      string                  `json:"artifactType,omitempty"`
	NativeGenerated   bool                    `json:"nativeGenerated,omitempty"`
	RuntimeVerified   bool                    `json:"runtimeVerified,omitempty"`
	BackupDirectory   string                  `json:"backupDirectory,omitempty"`
	ValidationStatus  string                  `json:"validationStatus,omitempty"`
	ID                string                  `json:"id"`
	Timestamp         string                  `json:"timestamp"`
	Target            string                  `json:"target"`
	PartIDs           []string                `json:"partIds"`
	PartNames         []string                `json:"partNames"`
	PartCount         int                     `json:"partCount"`
	Status            string                  `json:"status"`
	Directory         string                  `json:"directory"`
	Files             []string                `json:"files"`
	Warnings          []string                `json:"warnings"`
	Error             string                  `json:"error,omitempty"`
}

func cloneHistory(history []ExportHistoryEntry) []ExportHistoryEntry {
	out := make([]ExportHistoryEntry, len(history))
	for i, entry := range history {
		entry.PartIDs = append([]string{}, entry.PartIDs...)
		entry.PartNames = append([]string{}, entry.PartNames...)
		entry.Files = append([]string{}, entry.Files...)
		entry.Warnings = append([]string{}, entry.Warnings...)
		entry.ModelAssociations = cloneModelAssociations(entry.ModelAssociations)
		out[i] = entry
	}
	return out
}
func (s *Service) GetExportHistory() []ExportHistoryEntry {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return cloneHistory(s.history)
}
func (s *Service) ClearExportHistory() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if err := writeJSON(filepath.Join(s.dataDir, "history.json"), []ExportHistoryEntry{}); err != nil {
		return err
	}
	s.history = []ExportHistoryEntry{}
	return nil
}
func (s *Service) recordExport(req ExportRequest, result core.ExportResult, exportErr error) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	now := time.Now().UTC()
	entry := ExportHistoryEntry{ID: fmt.Sprintf("export-%d", now.UnixNano()), Timestamp: now.Format(time.RFC3339Nano), Target: req.Target, Status: "success", Directory: result.Directory, Files: append([]string{}, result.Files...), Warnings: append([]string{}, result.Warnings...), PartIDs: []string{}, PartNames: []string{}}
	entry.ArtifactType = result.ArtifactType
	entry.ModelAssociations = cloneModelAssociations(result.ModelAssociations)
	entry.NativeGenerated = result.NativeGenerated
	entry.RuntimeVerified = result.RuntimeVerified
	entry.BackupDirectory = result.BackupDirectory
	entry.ValidationStatus = result.ValidationStatus
	if entry.Target == "" {
		entry.Target = "kicad"
	}
	seen := map[string]bool{}
	for _, id := range req.PartIDs {
		if seen[id] {
			continue
		}
		seen[id] = true
		entry.PartIDs = append(entry.PartIDs, id)
		name := id
		if part, ok := s.parts[id]; ok {
			name = part.Name
		}
		entry.PartNames = append(entry.PartNames, name)
	}
	entry.PartCount = len(entry.PartIDs)
	if exportErr != nil {
		entry.Status = "error"
		entry.Error = exportErr.Error()
		entry.Directory = ""
		entry.Files = []string{}
	}
	history := append([]ExportHistoryEntry{entry}, s.history...)
	if len(history) > 100 {
		history = history[:100]
	}
	if err := writeJSON(filepath.Join(s.dataDir, "history.json"), history); err != nil {
		return err
	}
	s.history = history
	return nil
}

func (s *Service) ValidateGeneratedFile(path string) error {
	if !filepath.IsAbs(path) {
		return errors.New("只能定位已导出的文件")
	}
	path = filepath.Clean(path)
	s.mu.RLock()
	known := false
	for _, entry := range s.history {
		for _, file := range entry.Files {
			if strings.EqualFold(filepath.Clean(file), path) {
				known = true
				break
			}
		}
		if known {
			break
		}
	}
	s.mu.RUnlock()
	if !known {
		return errors.New("此文件不在导出历史中")
	}
	info, err := os.Stat(path)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return errors.New("导出文件已不存在或不是普通文件")
	}
	return nil
}

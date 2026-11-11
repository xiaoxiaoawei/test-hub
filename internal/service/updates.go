package service

import (
	"context"
	"errors"
	"os"
	"packagestudio/internal/updater"
	"path/filepath"
)

type updateClientState = updater.Client

func (s *Service) updater() *updater.Client {
	if s.updateClient == nil {
		s.updateClient = updater.NewClient(Version, filepath.Join(s.dataDir, "updates"))
	}
	return s.updateClient
}
func (s *Service) CheckForUpdates(ctx context.Context) (updater.Info, error) {
	s.updateMu.Lock()
	defer s.updateMu.Unlock()
	return s.updater().Check(ctx)
}
func (s *Service) DownloadUpdate(ctx context.Context) (updater.Download, error) {
	s.updateMu.Lock()
	defer s.updateMu.Unlock()
	return s.updater().Download(ctx)
}
func (s *Service) StartUpdate(path string) error {
	if !s.exportMu.TryLock() {
		return errors.New("导出任务进行中，请完成后再安装更新")
	}
	defer s.exportMu.Unlock()
	s.updateMu.Lock()
	defer s.updateMu.Unlock()
	exe, e := os.Executable()
	if e != nil {
		return e
	}
	helper, plan, e := s.updater().Prepare(path, exe, s.dataDir, os.Getpid())
	if e != nil {
		return e
	}
	return updater.StartHelper(helper, plan)
}
